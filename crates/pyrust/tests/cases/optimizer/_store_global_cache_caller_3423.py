# Helper for test_store_global_cache_3423.py: a second module that calls the
# owner module's global-writing functions and keeps same-named globals.
import _store_global_cache_owner_3423 as owner

COUNT = 1000
LABEL = "caller"


def drive(n):
    last = None
    for i in range(n):
        last = owner.bump(1)
    return last


def drive_direct(n):
    bump = owner.bump
    for i in range(n):
        bump(2)
    return owner.read()


def drive_method(n):
    tally = owner.Tally()
    for i in range(n):
        tally.bump(3)
    return owner.read()


def own_bump(n):
    global COUNT
    for i in range(n):
        COUNT += 1
    return COUNT
