# Chain terminal-history recovery

Terminal chain state and execution history are separate writes. A process can
stop after the terminal state commits and after history allocates an event
sequence, but before the event key is acknowledged. Retrying the old append
would allocate a different sequence and could duplicate the terminal event.

Each terminal chain revision now creates a durable receipt before its normal
history entry. The receipt contains the exact event payload, timestamp, and
allocated sequence. Its key includes the chain ID and the persisted state
version, so a later reset and terminal re-run receive an independent receipt.
If the normal event write is interrupted,
`Gateway::reconcile_chain_terminal_histories()` uses the receipt to finish the
original event key. Background cleanup runs that sweep automatically.

The event key is created only when absent. A lost acknowledgement after the
receipt write therefore produces one terminal event on recovery, and later
sweeps are no-ops. A failed receipt write leaves the terminal chain eligible for
reconstruction from its authoritative status on the next sweep.

## Evidence

The fault test interrupts the acknowledgement after a cancellation receipt is
stored. It verifies that history is initially missing the event, recovery writes
the receipted cancellation once with its original reason, and a second sweep
does not append another event.

## Remaining boundary

The chain-state store and history store are still not one transaction. A stop
before the receipt is persisted requires reconstruction from terminal state.
Other nonterminal history events retain their existing best-effort append
behavior.
