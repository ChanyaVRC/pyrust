# Helper for test_store_global_cache_3423.py: a filesystem module whose
# functions rebind its own globals while being called from other modules.
COUNT = 0
LABEL = "owner"

# Module-level rebinding in an imported module runs through StoreGlobal too.
ACC = 0
for _i in range(6):
    ACC = ACC + _i
SQUARES = []
for _i in range(4):
    SQUARES = SQUARES + [_i * _i]


def bump(step):
    global COUNT
    COUNT = COUNT + step
    return COUNT


def set_count(value):
    global COUNT
    COUNT = value


def read():
    return COUNT


def relabel(value):
    global LABEL
    LABEL = value


def read_label():
    return LABEL


def drop_and_rebind(value):
    global COUNT
    del COUNT
    COUNT = value


def define_late(value):
    global LATE
    LATE = value


def read_late():
    return LATE


class Tally:
    COUNT = "class attribute"

    def bump(self, step):
        global COUNT
        COUNT += step
        return COUNT
