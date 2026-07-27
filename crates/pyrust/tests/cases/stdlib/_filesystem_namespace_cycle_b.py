import _filesystem_namespace_cycle_a as peer

seen = (peer.value, peer.__dict__["value"])
peer.value = "b-write"
