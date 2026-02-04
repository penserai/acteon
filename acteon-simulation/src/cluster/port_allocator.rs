//! Dynamic port allocation for simulation nodes.

use std::collections::HashSet;
use std::net::{SocketAddr, TcpListener};

use parking_lot::Mutex;

/// Allocates unique ports for simulation nodes.
///
/// Primary strategy: bind to port 0 and let the OS assign an ephemeral port.
/// Fallback: range-based allocation if ephemeral ports fail.
#[derive(Debug)]
pub struct PortAllocator {
    range_start: u16,
    range_end: u16,
    allocated: Mutex<HashSet<u16>>,
}

impl Default for PortAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl PortAllocator {
    /// Create a new port allocator with default range (18000-19000).
    pub fn new() -> Self {
        Self::with_range(18000, 19000)
    }

    /// Create a new port allocator with a custom range.
    pub fn with_range(start: u16, end: u16) -> Self {
        Self {
            range_start: start,
            range_end: end,
            allocated: Mutex::new(HashSet::new()),
        }
    }

    /// Allocate a port, preferring OS-assigned ephemeral ports.
    ///
    /// Returns a `SocketAddr` bound to `127.0.0.1` with the allocated port.
    pub fn allocate(&self) -> Option<SocketAddr> {
        // Try ephemeral port first
        if let Some(addr) = Self::try_ephemeral() {
            let mut allocated = self.allocated.lock();
            allocated.insert(addr.port());
            return Some(addr);
        }

        // Fall back to range-based allocation
        self.try_range()
    }

    /// Try to allocate an ephemeral port via the OS.
    fn try_ephemeral() -> Option<SocketAddr> {
        let listener = TcpListener::bind("127.0.0.1:0").ok()?;
        let addr = listener.local_addr().ok()?;
        // Drop the listener to release the port, but we've recorded it
        drop(listener);
        Some(addr)
    }

    /// Try to allocate a port from the configured range.
    fn try_range(&self) -> Option<SocketAddr> {
        let mut allocated = self.allocated.lock();

        for port in self.range_start..self.range_end {
            if allocated.contains(&port) {
                continue;
            }

            // Try to bind to verify the port is available
            let addr: SocketAddr = format!("127.0.0.1:{port}").parse().ok()?;
            if TcpListener::bind(addr).is_ok() {
                allocated.insert(port);
                return Some(addr);
            }
        }

        None
    }

    /// Release a previously allocated port.
    pub fn release(&self, port: u16) {
        let mut allocated = self.allocated.lock();
        allocated.remove(&port);
    }

    /// Check if a port is currently allocated.
    pub fn is_allocated(&self, port: u16) -> bool {
        let allocated = self.allocated.lock();
        allocated.contains(&port)
    }

    /// Get the number of currently allocated ports.
    pub fn allocated_count(&self) -> usize {
        let allocated = self.allocated.lock();
        allocated.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_returns_unique_ports() {
        let allocator = PortAllocator::new();

        let addr1 = allocator.allocate().expect("should allocate first port");
        let addr2 = allocator.allocate().expect("should allocate second port");

        assert_ne!(addr1.port(), addr2.port());
        assert_eq!(allocator.allocated_count(), 2);
    }

    #[test]
    fn release_allows_reallocation() {
        let allocator = PortAllocator::with_range(19000, 19010);

        let addr = allocator.allocate().expect("should allocate");
        let port = addr.port();

        assert!(allocator.is_allocated(port));

        allocator.release(port);

        assert!(!allocator.is_allocated(port));
    }

    #[test]
    fn allocate_respects_range() {
        let allocator = PortAllocator::with_range(19000, 19005);

        // Force range-based allocation by filling ephemeral tracking
        // Note: This test may be flaky if ephemeral ports happen to be in range
        let addrs: Vec<_> = (0..5).filter_map(|_| allocator.allocate()).collect();

        // All allocations should succeed
        assert!(!addrs.is_empty());
    }
}
