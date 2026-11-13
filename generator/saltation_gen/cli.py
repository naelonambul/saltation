"""saltation-gen: seed customers and drive normal activity (stage 1)."""

import argparse
import asyncio
import json
import os
from dataclasses import asdict

from saltation_gen.client import BankClient, new_ulid
from saltation_gen.config import Settings
from saltation_gen.drivers import live_normal
from saltation_gen.drivers import seed as seeding
from saltation_gen.truth import TruthStore


async def _main(args: argparse.Namespace) -> None:
    settings = (
        Settings.from_env()
        if args.command in ("seed", "run-normal")
        else Settings(
            bank_url=os.environ.get("SALTATION_BANK_URL", "http://bank:8081"),
            truth_uri="",
            seed=int(os.environ.get("SALTATION_GEN_SEED", "20261112")),
        )
    )
    bank = BankClient(settings.bank_url)
    truth = None
    try:
        if args.command in ("seed", "run-normal"):
            truth = TruthStore(settings.truth_uri)
            await truth.setup()
        if args.command == "seed-parties":
            print(json.dumps(await seeding.seed_parties(bank, settings.seed, args.counterparties)))
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
