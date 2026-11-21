//! Exact-size, per-device buffer reuse. Only idle buffers enter the bounded cache.
use std::sync::{Arc, Mutex, Weak};

pub(crate) struct Pool {
    idle: Vec<wgpu::Buffer>,
    bytes: u64,
    max_bytes: u64,
    max_count: usize,
    #[cfg(test)]
    pub(crate) allocations: usize,
    #[cfg(test)]
    pub(crate) reuses: usize,
}

impl Pool {
    pub(crate) fn new(max_bytes: u64, max_count: usize) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            idle: Vec::new(),
            bytes: 0,
            max_bytes,
            max_count,
            #[cfg(test)]
            allocations: 0,
            #[cfg(test)]
            reuses: 0,
        }))
    }

    pub(crate) fn take(&mut self, device: &wgpu::Device, desc: &wgpu::BufferDescriptor<'_>) -> wgpu::Buffer {
        if let Some(index) = self.idle.iter().rposition(|b| b.size() == desc.size) {
            let buffer = self.idle.remove(index);
            self.bytes -= buffer.size();
            #[cfg(test)]
            {
                self.reuses += 1;
            }
            return buffer;
        }
        #[cfg(test)]
        {
            self.allocations += 1;
        }
        device.create_buffer(desc)
    }

    pub(crate) fn put(&mut self, buffer: wgpu::Buffer) {
        if buffer.size() > self.max_bytes || self.max_count == 0 {
            return;
        }
        while !self.idle.is_empty()
            && (self.bytes + buffer.size() > self.max_bytes || self.idle.len() >= self.max_count)
        {
            self.bytes -= self.idle.remove(0).size();
        }
        self.bytes += buffer.size();
        self.idle.push(buffer);
    }
}

/// A batch-private buffer cache layered over [`Pool`]. While a command encoder is recording
/// (`committed == false`), dropped intermediate frames return here so later passes in the same
/// encoder reuse them without exposing them to other threads. Once committed after `queue.submit()`,
/// idle buffers drain back to the shared pool and future frame drops return directly to it.
#[derive(Debug)]
pub(crate) struct BatchPool {
    state: Mutex<BatchPoolState>,
    parent: Weak<Mutex<Pool>>,
}

#[derive(Debug)]
struct BatchPoolState {
    idle: Vec<wgpu::Buffer>,
    committed: bool,
}

impl BatchPool {
    pub(crate) fn new(parent: &Arc<Mutex<Pool>>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(BatchPoolState { idle: Vec::new(), committed: false }),
            parent: Arc::downgrade(parent),
        })
    }

    pub(crate) fn take(&self, device: &wgpu::Device, desc: &wgpu::BufferDescriptor<'_>) -> wgpu::Buffer {
        {
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(index) = state.idle.iter().rposition(|b| b.size() == desc.size) {
                let buffer = state.idle.remove(index);
                #[cfg(test)]
                if let Some(parent) = self.parent.upgrade() {
                    parent.lock().unwrap_or_else(std::sync::PoisonError::into_inner).reuses += 1;
                }
                return buffer;
            }
        }
        if let Some(parent) = self.parent.upgrade() {
            parent.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take(device, desc)
        } else {
            device.create_buffer(desc)
        }
    }

    fn release(&self, buffer: wgpu::Buffer) {
        let committed = {
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.committed {
                true
            } else {
                state.idle.push(buffer.clone());
                false
            }
        };
        if committed {
            if let Some(parent) = self.parent.upgrade() {
                parent.lock().unwrap_or_else(std::sync::PoisonError::into_inner).put(buffer);
            }
        }
    }

    pub(crate) fn commit(&self) {
        let drained = {
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.committed {
                return;
            }
            state.committed = true;
            std::mem::take(&mut state.idle)
        };
        if !drained.is_empty() {
            if let Some(parent) = self.parent.upgrade() {
                let mut pool = parent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                for buffer in drained {
                    pool.put(buffer);
                }
            }
        }
    }
}

/// A frame clone keeps the lease alive. Pending dispatches are already submitted to the same queue before
/// the last clone drops, so their reads precede any future pooled write without a host synchronization.
#[derive(Debug)]
pub(crate) struct Lease {
    buffer: wgpu::Buffer,
    batch: Arc<BatchPool>,
}

impl Lease {
    pub(crate) fn from_batch(buffer: &wgpu::Buffer, batch: &Arc<BatchPool>) -> Arc<Self> {
        Arc::new(Self { buffer: buffer.clone(), batch: Arc::clone(batch) })
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.batch.release(self.buffer.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AdapterKind, Gpu};

    #[test]
    fn idle_cache_is_bounded_and_exact_size() {
        let Some(g) = Gpu::with_adapter(AdapterKind::Hardware).expect("GPU initialization") else {
            eprintln!("SKIP cache-bound test: no adapter");
            return;
        };
        let pool = Pool::new(128, 2);
        let mut pool = pool.lock().unwrap();
        let descriptor = |size| wgpu::BufferDescriptor {
            label: Some("pool test"),
            size,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        };
        for size in [32, 48, 64] {
            let buffer = pool.take(&g.device, &descriptor(size));
            pool.put(buffer);
        }
        assert_eq!(pool.bytes, 112);
        assert_eq!(pool.idle.len(), 2);
        let buffer = pool.take(&g.device, &descriptor(64));
        assert_eq!(buffer.size(), 64);
        assert_eq!(pool.reuses, 1);
        let oversized = pool.take(&g.device, &descriptor(256));
        pool.put(oversized);
        assert_eq!(pool.bytes, 48);
        assert_eq!(pool.idle.len(), 1);
    }
}
