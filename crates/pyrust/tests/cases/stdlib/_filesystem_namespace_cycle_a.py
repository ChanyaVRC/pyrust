value = "a-before"

import _filesystem_namespace_cycle_b as peer

seen_by_peer = peer.seen
seen_after_peer_write = value
