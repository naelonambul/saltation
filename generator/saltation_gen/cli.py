"""saltation-gen: seed customers and drive normal activity (stage 1)."""

import argparse
import asyncio
import json
import os
import signal
from dataclasses import asdict
from datetime import date, datetime

from saltation_gen.client import BankClient, new_ulid
from saltation_gen.config import Settings
from saltation_gen.config.stage6 import ScheduleConfig
from saltation_gen.drivers import atm, bc, behavior, cards, live_normal, stage6
from saltation_gen.drivers import seed as seeding
from saltation_gen.truth import TruthStore
from saltation_gen.truth.recovery import RecoveryError

STAGE6_COMMANDS = ("run-backfill", "run-live", "run-backfill-live")
TRUTH_COMMANDS = (
    "seed",
    "run-normal",
    "atm-smoke",
    "generator-bc",
    "generator-cards",
    "generator-behavior",
    *STAGE6_COMMANDS,
)


def _quota_map(value: str) -> dict[str, int]:
    try:
        parsed = json.loads(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("quotas must be a JSON object") from error
    if not isinstance(parsed, dict) or any(type(count) is not int or count < 0 for count in parsed.values()):
        raise argparse.ArgumentTypeError("quotas must be a JSON object with nonnegative integer counts")
    return parsed


async def _stage6(args: argparse.Namespace, bank: BankClient, truth: TruthStore, seed: int) -> None:
    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    signals = (signal.SIGINT, signal.SIGTERM)
    for signum in signals:
        loop.add_signal_handler(signum, stop.set)
    try:
        result = await stage6.run_stage6(
            bank,
            truth,
            args.run_id,
            seed,
            ScheduleConfig(
                args.customers, args.amplification, args.annual_quotas, args.live_quotas, args.normal
            ),
            stage6.RunConfig(
                first=args.first,
                days=args.days,
                live_at=args.live_at,
                live_seconds=args.live_seconds,
                workers=args.workers,
                bucket_seconds=args.bucket_seconds,
                start=args.start,
            ),
            args.command.removeprefix("run-"),
            stop,
            resume=args.resume,
            exited_owner=args.recover_exited_owner,
        )
    except (RecoveryError, ValueError) as error:
        raise SystemExit(str(error)) from None
    finally:
        for signum in signals:
            loop.remove_signal_handler(signum)
    print(json.dumps(result))


async def _main(args: argparse.Namespace) -> None:
    settings = (
        Settings.from_env()
        if args.command in TRUTH_COMMANDS
        else Settings(
            bank_url=os.environ.get("SALTATION_BANK_URL", "http://bank:8081"),
            truth_uri="",
            seed=int(os.environ.get("SALTATION_GEN_SEED", "20261112")),
        )
    )
    bank = BankClient(settings.bank_url)
    truth = None
    try:
        if args.command in TRUTH_COMMANDS:
            truth = TruthStore(settings.truth_uri)
            if args.command != "atm-smoke":
                await truth.setup()
        if args.command in STAGE6_COMMANDS:
            await _stage6(args, bank, truth, settings.seed)
        elif args.command == "seed-parties":
            print(json.dumps(await seeding.seed_parties(bank, settings.seed, args.counterparties)))
        elif args.command == "atm-smoke":
            print(json.dumps(await atm.smoke(bank, truth)))
        elif args.command == "generator-bc":
            try:
                result = await bc.run(
                    bank,
                    truth,
                    args.run_id,
                    args.sender_account,
                    args.receiver_account,
                    args.transfer_amount,
                    args.withdraw_amount,
                    exited_owner=args.recover_exited_owner,
                )
            except RecoveryError as error:
                raise SystemExit(str(error)) from None
            print(json.dumps(result))
        elif args.command in ("generator-cards", "generator-behavior"):
            driver = cards if args.command == "generator-cards" else behavior
            try:
                result = await driver.run(
                    bank,
                    truth,
                    args.run_id,
                    settings.seed,
                    args.date,
                    exited_owner=args.recover_exited_owner,
                )
            except RecoveryError as error:
                raise SystemExit(str(error)) from None
            print(json.dumps(result))
        elif args.command == "seed-terminals":
            print(json.dumps({"atm_terminals": await seeding.seed_terminals(bank)}))
        elif args.command == "clock":
            if args.clock_command == "show":
                result = await bank.clock()
            elif args.clock_command == "set":
                result = await bank.clock_set(args.business_time)
            elif args.clock_command == "advance":
                result = await bank.clock_advance(args.seconds)
            else:
                result = await bank.go_live()
            print(json.dumps(result))
        if args.command == "seed":
            n = await seeding.seed(bank, truth, settings.seed, args.customers, args.start)
            print(json.dumps({"seeded": n}))
        elif args.command == "run-normal":
            run_id = new_ulid()
            await truth.start_run(
                run_id,
                kind="normal",
                seed=settings.seed,
                minutes=args.minutes,
                rate=args.rate,
                amplification=1,
            )
            stats = await live_normal.run(bank, truth, run_id, settings.seed, args.minutes, args.rate)
            await truth.finish_run(
                run_id, **asdict(stats) | {"statuses": {str(k): v for k, v in stats.statuses.items()}}
            )
            print(json.dumps({"run_id": run_id, **asdict(stats)}, default=str))
    finally:
        await bank.close()
        if truth is not None:
            await truth.close()


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="saltation-gen")
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("seed", help="seed customers with funded demand accounts")
    p.add_argument("--customers", type=int, required=True)
    p.add_argument("--start", type=int, default=0)
    p = sub.add_parser("run-normal", help="normal transfers at a fixed rate")
    p.add_argument("--minutes", type=float, required=True)
    p.add_argument("--rate", type=float, default=5.0, help="requests per second")
    p = sub.add_parser("seed-parties")
    p.add_argument("--counterparties", type=int, required=True)
    sub.add_parser("seed-terminals")
    sub.add_parser("atm-smoke")
    p = sub.add_parser("generator-bc", help="run or resume a synthetic B/C proof against a frozen bank")
    p.add_argument("--run-id", required=True)
    p.add_argument("--sender-account", required=True)
    p.add_argument("--receiver-account", required=True)
    p.add_argument("--transfer-amount", type=int, required=True)
    p.add_argument("--withdraw-amount", type=int, required=True)
    p.add_argument(
        "--recover-exited-owner",
        metavar="OWNER",
        help="replace this exact stale owner only after verifying its process has exited",
    )
    for command in ("generator-cards", "generator-behavior"):
        p = sub.add_parser(command, help="run a seeded fixture on a dedicated backfill bank")
        p.add_argument("--run-id", required=True)
        p.add_argument("--date", type=date.fromisoformat, required=True, help="fixture KST business date")
        p.add_argument(
            "--recover-exited-owner",
            metavar="OWNER",
            help="replace this exact stale owner only after verifying its process has exited",
        )
    for command in STAGE6_COMMANDS:
        p = sub.add_parser(command, help="drive a journaled workload on a dedicated Stage 6 bank")
        p.add_argument("--run-id", required=True)
        p.add_argument("--first", required=True, type=date.fromisoformat, help="first KST workload date")
        p.add_argument("--days", type=int, default=365)
        p.add_argument(
            "--live-at", type=datetime.fromisoformat, help="planned cutover with explicit timezone"
        )
        p.add_argument("--live-seconds", type=int, default=86_400)
        p.add_argument("--customers", type=int, default=20_000)
        p.add_argument("--start", type=int, default=0, help="first synthetic customer slot")
        p.add_argument("--amplification", type=int, default=20)
        p.add_argument("--annual-quotas", type=_quota_map, help="JSON object for scaled proof quotas")
        p.add_argument("--live-quotas", type=_quota_map, help="JSON object for scaled proof quotas")
        p.add_argument("--normal", action=argparse.BooleanOptionalAction, default=True)
        p.add_argument("--workers", type=int, default=8)
        p.add_argument("--bucket-seconds", type=int, default=60)
        p.add_argument("--resume", action="store_true")
        p.add_argument("--recover-exited-owner", metavar="OWNER")
    p = sub.add_parser("clock")
    clocks = p.add_subparsers(dest="clock_command", required=True)
    clocks.add_parser("show")
    clocks.add_parser("set").add_argument("business_time")
    clocks.add_parser("advance").add_argument("seconds", type=int)
    clocks.add_parser("live")
    return parser


def main() -> None:
    asyncio.run(_main(build_parser().parse_args()))


if __name__ == "__main__":
    main()
