# Helper module for test_circular_import.py (issue #2727). Not a test entry
# point (name does not start with "test_"), so the parity harness imports it
# rather than running it directly.
import sys

print("a: 'a' in sys.modules during a's body:", "_circ_a" in sys.modules)
import _circ_b

print("a: back in a after importing b")
a_value = "a_done"
