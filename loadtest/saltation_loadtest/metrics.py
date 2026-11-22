"""Prometheus exposition parser and histogram delta quantile calculator."""

from __future__ import annotations

import math
import re
from dataclasses import dataclass, field

_SAMPLE_RE = re.compile(
    r"^([a-zA-Z_:][a-zA-Z0-9_:]*)(?:\{([^}]*)\})?\s+([+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?|[+-]?Inf|NaN)$"
)
_LABEL_RE = re.compile(r'([a-zA-Z_][a-zA-Z0-9_]*)="((?:\\.|[^"\\])*)"')


@dataclass
class Sample:
    name: str
    labels: dict[str, str]
    value: float


@dataclass
class MetricsSnapshot:
    samples: list[Sample] = field(default_factory=list)

    @classmethod
    def parse(cls, text: str) -> MetricsSnapshot:
        samples: list[Sample] = []
        for raw_line in text.splitlines():
            line = raw_line.strip()
            if not line or line.startswith("#"):
                continue
            m = _SAMPLE_RE.match(line)
            if not m:
                continue
            name, label_str, val_str = m.groups()
            labels: dict[str, str] = {}
            if label_str:
                for k, v in _LABEL_RE.findall(label_str):
                    labels[k] = v.replace(r"\"", '"').replace(r"\\", "\\")
            try:
                val = float(val_str)
            except ValueError:
                continue
            if math.isnan(val):
                continue
            samples.append(Sample(name=name, labels=labels, value=val))
        return cls(samples=samples)

    def counter_sum(self, name: str, match: dict[str, str] | None = None) -> float:
        total = 0.0
        for s in self.samples:
            if s.name != name:
                continue
            if match and any(s.labels.get(k) != v for k, v in match.items()):
                continue
            total += s.value
        return total

    def gauge_max(self, name: str, match: dict[str, str] | None = None) -> float:
        vals = [
            s.value
            for s in self.samples
            if s.name == name and (not match or all(s.labels.get(k) == v for k, v in match.items()))
        ]
        return max(vals) if vals else 0.0

    def histogram_buckets(self, base_name: str, match: dict[str, str] | None = None) -> dict[float, float]:
        """Aggregates cumulative bucket counts across matching series."""
        bucket_name = f"{base_name}_bucket"
        buckets: dict[float, float] = {}
        for s in self.samples:
            if s.name != bucket_name:
                continue
            if match and any(s.labels.get(k) != v for k, v in match.items()):
                continue
            le_str = s.labels.get("le")
            if le_str is None:
                continue
            le = math.inf if le_str == "+Inf" else float(le_str)
            buckets[le] = buckets.get(le, 0.0) + s.value
        return buckets

    def histogram_sum_count(self, base_name: str, match: dict[str, str] | None = None) -> tuple[float, float]:
        s_val = self.counter_sum(f"{base_name}_sum", match)
        c_val = self.counter_sum(f"{base_name}_count", match)
        return s_val, c_val


def histogram_delta_buckets(
    before: MetricsSnapshot, after: MetricsSnapshot, base_name: str, match: dict[str, str] | None = None
) -> dict[float, float]:
    def series(snapshot: MetricsSnapshot) -> dict[tuple, dict[float, float]]:
        results: dict[tuple, dict[float, float]] = {}
        for sample in snapshot.samples:
            if match and any(sample.labels.get(k) != v for k, v in match.items()):
                continue
            if sample.name == f"{base_name}_bucket":
                try:
                    bound = float(sample.labels["le"])
                except (KeyError, ValueError) as exc:
                    raise ValueError("invalid histogram bucket bound") from exc
            elif sample.name == f"{base_name}_count":
                bound = -1.0
            else:
                continue
            key = tuple(sorted((k, v) for k, v in sample.labels.items() if k != "le"))
            values = results.setdefault(key, {})
            if bound in values or not math.isfinite(sample.value) or sample.value < 0:
                raise ValueError("invalid histogram series")
            values[bound] = sample.value
        return results

    initial, final = series(before), series(after)
    if initial.keys() - final.keys():
        raise ValueError("histogram series disappeared")
    cumulative: dict[float, float] = {}
    expected_bounds = None
    for key, current in final.items():
        previous = initial.get(key, {})
        if (previous and previous.keys() != current.keys()) or (
            expected_bounds is not None and current.keys() != expected_bounds
        ):
            raise ValueError("histogram bounds changed")
        expected_bounds = current.keys()
        delta = {bound: value - previous.get(bound, 0) for bound, value in current.items()}
        count = delta.pop(-1.0, None)
        if count is None or count != delta.get(math.inf):
            raise ValueError("histogram count does not match buckets")
        last = 0.0
        for bound, value in sorted(delta.items()):
            if bound <= 0 or math.isnan(bound) or value < last or not value.is_integer():
                raise ValueError("histogram counter reset or invalid bucket")
            cumulative[bound] = cumulative.get(bound, 0.0) + value
            last = value
    return cumulative


def histogram_quantile(q: float, buckets: dict[float, float]) -> float | None:
    """Prometheus-style linear interpolation over cumulative histogram buckets."""
    if not buckets:
        return None
    ordered = sorted(buckets.items(), key=lambda kv: kv[0])
    total = ordered[-1][1]
    if total <= 0:
        return None
    rank = q * total
    prev_bound = 0.0
    prev_count = 0.0
    for bound, count in ordered:
        if count >= rank:
            if math.isinf(bound):
                return None
            bucket_count = count - prev_count
            if bucket_count <= 0:
                return bound
            fraction = (rank - prev_count) / bucket_count
            return prev_bound + (bound - prev_bound) * fraction
        prev_bound = bound
        prev_count = count
    return prev_bound


@dataclass
class ServerWindowMetrics:
    transfer_count: int
    transfer_p50_ms: float | None
    transfer_p95_ms: float | None
    transfer_p99_ms: float | None
    lock_wait_count: int
    lock_wait_p95_ms: float | None
    lock_wait_mean_ms: float | None
    deadlock_retries: int
    outbox_full_events: int
    outbox_relay_errors: int
    outbox_undelivered_end: int
    daily_close_mismatches: int
    limiting_resource: str
    pool_saturated: bool = False
    status_counts: dict[int, int] = field(default_factory=dict)


def summarize_window(before: MetricsSnapshot, after: MetricsSnapshot) -> ServerWindowMetrics:
    tx_match = {"listener": "GENERATOR", "endpoint": "/v2.0/transfer/withdraw/fin_num"}
    tx_buckets = histogram_delta_buckets(before, after, "bank_http_request_duration_seconds", tx_match)
    _, tx_c0 = before.histogram_sum_count("bank_http_request_duration_seconds", tx_match)
    _, tx_c1 = after.histogram_sum_count("bank_http_request_duration_seconds", tx_match)
    tx_count = max(0, round(tx_c1 - tx_c0))
    statuses = {
        int(sample.labels.get("status", ""))
        for sample in after.samples
        if sample.name == "bank_http_request_duration_seconds_count"
        and all(sample.labels.get(k) == v for k, v in tx_match.items())
    }
    if any(not 100 <= status <= 599 for status in statuses):
        raise ValueError("invalid HTTP metrics status")
    status_counts = {
        status: int(
            histogram_delta_buckets(
                before, after, "bank_http_request_duration_seconds", {**tx_match, "status": str(status)}
            )[math.inf]
        )
        for status in statuses
    }

    p50_s = histogram_quantile(0.50, tx_buckets)
    p95_s = histogram_quantile(0.95, tx_buckets)
    p99_s = histogram_quantile(0.99, tx_buckets)

    lock_match = {"statement": "ledger.lock_accounts"}
    lock_buckets = histogram_delta_buckets(before, after, "bank_db_statement_duration_seconds", lock_match)
    lock_s0, lock_c0 = before.histogram_sum_count("bank_db_statement_duration_seconds", lock_match)
    lock_s1, lock_c1 = after.histogram_sum_count("bank_db_statement_duration_seconds", lock_match)
    lock_count = max(0, round(lock_c1 - lock_c0))
    lock_p95_s = histogram_quantile(0.95, lock_buckets)
    lock_mean_ms = ((lock_s1 - lock_s0) / lock_count * 1000.0) if lock_count > 0 else None

    deadlocks = max(
        0,
        round(
            after.counter_sum("bank_deadlock_retries_total")
            - before.counter_sum("bank_deadlock_retries_total")
        ),
    )
    outbox_full = max(
        0,
        round(after.counter_sum("bank_outbox_full_total") - before.counter_sum("bank_outbox_full_total")),
    )
    relay_err = max(
        0,
        round(
            after.counter_sum("bank_outbox_relay_errors_total")
            - before.counter_sum("bank_outbox_relay_errors_total")
        ),
    )
    outbox_end = round(after.gauge_max("bank_outbox_undelivered"))
    close_mismatches = round(after.gauge_max("bank_daily_close_mismatches"))

    p95_ms = p95_s * 1000.0 if p95_s is not None else None
    lock_p95_ms = lock_p95_s * 1000.0 if lock_p95_s is not None else None
    pool_capacity = after.gauge_max("bank_db_pool_capacity")
    pool_buckets = histogram_delta_buckets(before, after, "bank_db_pool_acquire_wait_seconds")
    pool_wait = histogram_quantile(0.95, pool_buckets)
    pool_saturated = (
        pool_capacity > 0
        and pool_wait is not None
        and p95_s is not None
        and pool_wait >= max(0.05, 0.4 * p95_s)
    )
    limiting = classify_limiting_resource(
        p95_ms, lock_p95_ms, deadlocks, outbox_full, pool_saturated=pool_saturated
    )

    return ServerWindowMetrics(
        transfer_count=tx_count,
        transfer_p50_ms=p50_s * 1000.0 if p50_s is not None else None,
        transfer_p95_ms=p95_ms,
        transfer_p99_ms=p99_s * 1000.0 if p99_s is not None else None,
        lock_wait_count=lock_count,
        lock_wait_p95_ms=lock_p95_ms,
        lock_wait_mean_ms=lock_mean_ms,
        deadlock_retries=deadlocks,
        outbox_full_events=outbox_full,
        outbox_relay_errors=relay_err,
        outbox_undelivered_end=outbox_end,
        daily_close_mismatches=close_mismatches,
        limiting_resource=limiting,
        pool_saturated=pool_saturated,
        status_counts=status_counts,
    )


def classify_limiting_resource(
    transfer_p95_ms: float | None,
    lock_wait_p95_ms: float | None,
    deadlock_retries: int,
    outbox_full_events: int,
    *,
    pool_saturated: bool = False,
) -> str:
    if outbox_full_events > 0:
        return "outbox_tablespace"
    if deadlock_retries > 0 or (
        lock_wait_p95_ms is not None
        and transfer_p95_ms is not None
        and transfer_p95_ms > 0
        and lock_wait_p95_ms >= 0.4 * transfer_p95_ms
    ):
        return "row_lock_wait"
    if transfer_p95_ms is not None and transfer_p95_ms > 300.0:
        return "db_pool" if pool_saturated else "unknown"
    return "none_saturated"
