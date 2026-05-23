package com.acteon.client;

import com.acteon.client.exceptions.ActeonException;

import java.util.Iterator;
import java.util.NoSuchElementException;

/**
 * Wraps a sequence of {@link BusSseIterator}s with best-effort
 * exponential backoff. When the inner iterator drains (the server
 * closed the SSE stream), this wrapper sleeps according to the
 * {@link Bus.ReconnectConfig}, opens a fresh subscription from
 * {@code latest}, and yields a {@link Bus.BusConsumeItem.Reconnected}
 * boundary item before the next live record.
 *
 * <p>Resume is always from {@code latest} because the Phase 1 bus
 * subscribe handler has no per-partition offset seek. Workloads
 * that need lossless delivery should use Phase 2 durable
 * subscriptions with manual ack instead.</p>
 *
 * <p>The attempt counter resets after a successful read so a long-
 * stable connection isn't penalised for a single later blip.</p>
 */
public final class ReconnectingBusSseIterator
    implements Iterator<Bus.BusConsumeItem>, AutoCloseable {

    /** Opens a fresh inner iterator. */
    @FunctionalInterface
    public interface InnerOpener {
        BusSseIterator open(boolean firstAttempt) throws ActeonException;
    }

    private final InnerOpener opener;
    private final Bus.ReconnectConfig config;
    private BusSseIterator current;
    private Bus.BusConsumeItem nextItem;
    private boolean closed;
    private int attempt;
    /** When set, emitted before pulling the next live item. */
    private Bus.BusConsumeItem.Reconnected pendingReconnected;

    public ReconnectingBusSseIterator(InnerOpener opener, Bus.ReconnectConfig config)
        throws ActeonException {
        this.opener = opener;
        this.config = config;
        this.current = opener.open(true);
    }

    @Override
    public boolean hasNext() {
        if (closed) return false;
        if (nextItem != null) return true;
        nextItem = readNext();
        return nextItem != null;
    }

    @Override
    public Bus.BusConsumeItem next() {
        if (!hasNext()) {
            throw new NoSuchElementException("No more bus SSE items");
        }
        Bus.BusConsumeItem item = nextItem;
        nextItem = null;
        return item;
    }

    @Override
    public void close() {
        closed = true;
        if (current != null) {
            current.close();
            current = null;
        }
    }

    private Bus.BusConsumeItem readNext() {
        while (!closed) {
            if (pendingReconnected != null) {
                Bus.BusConsumeItem item = pendingReconnected;
                pendingReconnected = null;
                return item;
            }
            if (current == null) {
                // Last reconnect attempt didn't open a stream; sleep
                // again and try once more. `attemptReconnect` returns
                // false only when we've hit `maxAttempts` or been
                // interrupted.
                if (!attemptReconnect()) return null;
                continue;
            }
            try {
                if (current.hasNext()) {
                    Bus.BusConsumeItem item = current.next();
                    attempt = 0;
                    return item;
                }
            } catch (RuntimeException e) {
                // Mid-stream parse failure — surface as a typed
                // error and trigger a reconnect on the next call.
                current.close();
                current = null;
                return new Bus.BusConsumeItem.Error(e.getMessage());
            }
            // Inner iterator drained cleanly. Tear it down and let
            // the loop top take care of opening a fresh one.
            current.close();
            current = null;
        }
        return null;
    }

    /**
     * Sleep for the current backoff, bump the attempt counter, then
     * try to open a new inner iterator. Always queues a
     * {@link Bus.BusConsumeItem.Reconnected} boundary so callers
     * observe the attempt even if the open call fails.
     *
     * @return false when {@code maxAttempts} is exhausted or the
     *     thread was interrupted; true otherwise.
     */
    private boolean attemptReconnect() {
        if (config.maxAttempts() > 0 && attempt >= config.maxAttempts()) {
            close();
            return false;
        }
        long backoffMs = backoffFor(attempt, config);
        try {
            Thread.sleep(backoffMs);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            close();
            return false;
        }
        attempt++;
        pendingReconnected = new Bus.BusConsumeItem.Reconnected(backoffMs, attempt);
        try {
            current = opener.open(false);
        } catch (ActeonException e) {
            // Open failed; we still emit the Reconnected boundary so
            // callers see the attempt counter, then loop back and
            // retry on the next call (with another sleep).
            current = null;
        }
        return true;
    }

    static long backoffFor(int attempt, Bus.ReconnectConfig config) {
        int shift = Math.min(attempt, 20);
        long exp = config.initialBackoffMs() * (1L << shift);
        return Math.min(exp, config.maxBackoffMs());
    }
}
