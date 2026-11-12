import random
import re

import pytest

from saltation_gen.client import bank_tran_id, new_ulid
from saltation_gen.drivers.seed import make_customers
from saltation_gen.personas.normal import home_context, next_step
from saltation_gen.truth import Label

CROCKFORD = re.compile(r"^[0-9A-HJKMNP-TV-Z]{26}$")


def test_ulids_are_valid_and_monotonic():
    ids = [new_ulid() for _ in range(1000)]
    assert all(CROCKFORD.match(i) for i in ids)
    assert ids == sorted(ids)
    assert len(set(ids)) == len(ids)


def test_bank_tran_id_shape():
    assert re.match(r"^[A-Za-z0-9]{10}U[A-Z0-9]{9}$", bank_tran_id())


def test_seeding_is_deterministic():
    a = make_customers(7, 5)
    assert a == make_customers(7, 5)
    assert a[0]["customer_no"] == "C00000000" and a[0]["account_no"] == "99800000000"
    assert len({c["phone"] for c in make_customers(7, 100)}) == 100


def test_home_context_is_stable_per_customer():
    c = {"customer_no": "C00000001"}
    assert home_context(c) == home_context(c)


def test_next_step_never_sends_to_self():
    roster = [{"customer_no": f"C{i}", "account_no": f"998{i}"} for i in range(5)]
    rng = random.Random(1)
    for _ in range(200):
        step = next_step(rng, roster)
        assert step.sender["customer_no"] != step.receiver["customer_no"]


def test_label_validation():
    Label("r", "account", "998", "normal", "run")
    with pytest.raises(ValueError):
        Label("r", "device", "x", "normal", "run")
    with pytest.raises(ValueError):
        Label("r", "account", "x", "Z", "run")
