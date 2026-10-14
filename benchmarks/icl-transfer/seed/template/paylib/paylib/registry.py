"""Реестр публичных операций paylib. Owner: payments-core"""
from paylib import money

REGISTRY = {
    "money.round_money": money.round_money,
    "money.to_money": money.to_money,
}
