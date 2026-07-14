//! Platform-neutral pieces of the multi-device fan-out engine (ring
//! buffering and format math), kept free of OS dependencies so their
//! logic is unit-testable on every platform. Consumed by the Windows
//! WASAPI engine.

use std::collections::VecDeque;
use std::sync::Mutex;

use log::debug;

// ---------------------------------------------------------------------------
// Format math
// ---------------------------------------------------------------------------

/// The two facts about a PCM byte stream that all byte/frame math needs:
/// frame size (`nBlockAlign`) and byte rate (`nAvgBytesPerSec`).
#[derive(Debug, Clone, Copy)]
pub struct FrameLayout {
    /// Bytes per frame; frames are never split.
    pub block_align: usize,
    /// Bytes per second of audio.
    pub avg_bytes_per_sec: usize,
}

impl FrameLayout {
    /// Byte count of `ms` milliseconds of audio, rounded down to whole frames
    /// (at least one frame).
    pub fn bytes_for_ms(&self, ms: usize) -> usize {
        let frames = (self.avg_bytes_per_sec * ms / 1000 / self.block_align).max(1);
        frames * self.block_align
    }
}

// ---------------------------------------------------------------------------
// Ring buffer
// ---------------------------------------------------------------------------

/// Per-secondary FIFO of raw capture-format bytes. A mutex-guarded VecDeque:
/// correctness over cleverness — the lock is held for microseconds every
/// ~10 ms, so a lock-free SPSC ring is a later optimization, not a need.
pub struct Ring {
    buf: Mutex<VecDeque<u8>>,
    target_bytes: usize,
    max_bytes: usize,
    frame_size: usize,
}

impl Ring {
    /// A ring that steadies around `target_ms` of buffered audio and, once
    /// occupancy exceeds `max_ms`, clamps back down to the target.
    pub fn new(layout: &FrameLayout, target_ms: usize, max_ms: usize) -> Self {
        let target_bytes = layout.bytes_for_ms(target_ms);
        let max_bytes = layout.bytes_for_ms(max_ms);
        Self {
            buf: Mutex::new(VecDeque::with_capacity(max_bytes + layout.block_align)),
            target_bytes,
            max_bytes,
            frame_size: layout.block_align,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<u8>> {
        // A poisoned lock only means a peer thread panicked mid-copy; the
        // byte queue itself is still structurally sound.
        self.buf.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn push(&self, data: &[u8]) {
        let mut buf = self.lock();
        buf.extend(data);
        if buf.len() > self.max_bytes {
            // Drift/overflow: the sink drains slower than the source fills.
            // Drop the oldest whole frames back to the target occupancy.
            let excess = buf.len() - self.target_bytes;
            let drop = excess / self.frame_size * self.frame_size;
            buf.drain(..drop);
            debug!(
                "ring above {} bytes; dropped {drop} bytes of oldest audio",
                self.max_bytes
            );
        }
    }

    /// Move as many whole frames as fit into `out`; returns bytes written.
    pub fn pop_into(&self, out: &mut [u8]) -> usize {
        let mut buf = self.lock();
        let avail = buf.len() / self.frame_size * self.frame_size;
        let take = avail.min(out.len() / self.frame_size * self.frame_size);
        if take == 0 {
            return 0;
        }
        let (front, back) = buf.as_slices();
        if take <= front.len() {
            out[..take].copy_from_slice(&front[..take]);
        } else {
            out[..front.len()].copy_from_slice(front);
            out[front.len()..take].copy_from_slice(&back[..take - front.len()]);
        }
        buf.drain(..take);
        take
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1000 B/s makes `bytes_for_ms(ms)` read as "`ms` bytes rounded down to
    /// frames", keeping ring sizes in the tests small and literal.
    fn layout(block_align: usize) -> FrameLayout {
        FrameLayout {
            block_align,
            avg_bytes_per_sec: 1000,
        }
    }

    /// Bytes carrying their own index, so FIFO order and drop offsets are
    /// visible in assertions.
    fn counter_bytes(range: std::ops::Range<u8>) -> Vec<u8> {
        range.collect()
    }

    /// Next `n` bytes of the wrapping counter sequence at `next`.
    fn feed(next: &mut u8, n: usize) -> Vec<u8> {
        (0..n)
            .map(|_| {
                let b = *next;
                *next = next.wrapping_add(1);
                b
            })
            .collect()
    }

    /// Assert `got` continues the wrapping counter sequence at `next`.
    fn expect(next: &mut u8, got: &[u8]) {
        for &b in got {
            assert_eq!(b, *next);
            *next = next.wrapping_add(1);
        }
    }

    // --- FrameLayout::bytes_for_ms ---

    #[test]
    fn bytes_for_ms_rounds_down_to_whole_frames() {
        // 48 kHz stereo 16-bit: 4-byte frames, 192 kB/s.
        let l = FrameLayout {
            block_align: 4,
            avg_bytes_per_sec: 192_000,
        };
        assert_eq!(l.bytes_for_ms(60), 11_520); // exactly 60 ms, frame-aligned
        // 7 ms at 1000 B/s is 7 bytes; only one whole 6-byte frame fits.
        assert_eq!(layout(6).bytes_for_ms(7), 6);
    }

    #[test]
    fn bytes_for_ms_returns_at_least_one_frame() {
        assert_eq!(layout(4).bytes_for_ms(0), 4); // ms = 0
        assert_eq!(layout(4).bytes_for_ms(1), 4); // 1 byte rounds up to a frame
    }

    #[test]
    fn bytes_for_ms_handles_degenerate_layouts() {
        // Mono 8-bit: every byte is a frame.
        let l = FrameLayout {
            block_align: 1,
            avg_bytes_per_sec: 1234,
        };
        assert_eq!(l.bytes_for_ms(10), 12); // 12.34 bytes -> 12
        assert_eq!(l.bytes_for_ms(0), 1);
        // 32ch float at 384 kHz for two minutes: no overflow, exact result.
        let l = FrameLayout {
            block_align: 128,
            avg_bytes_per_sec: 49_152_000,
        };
        assert_eq!(l.bytes_for_ms(120_000), 49_152_000 * 120);
    }

    // --- Ring: underrun ---

    #[test]
    fn pop_on_empty_ring_returns_zero_and_leaves_out_untouched() {
        let ring = Ring::new(&layout(4), 60, 120);
        let mut out = [0xAA; 8];
        assert_eq!(ring.pop_into(&mut out), 0);
        // The render loop zero-fills `out[filled..]` itself; pop must not
        // have scribbled on the part it did not fill.
        assert_eq!(out, [0xAA; 8]);
    }

    #[test]
    fn pop_with_less_than_one_frame_buffered_or_requested_returns_zero() {
        let ring = Ring::new(&layout(4), 60, 120);
        ring.push(&counter_bytes(0..2)); // half a frame buffered
        assert_eq!(ring.pop_into(&mut [0u8; 16]), 0);
        ring.push(&counter_bytes(2..6)); // now one whole frame + half
        assert_eq!(ring.pop_into(&mut [0u8; 3]), 0); // sub-frame output
        assert_eq!(ring.pop_into(&mut []), 0); // empty output
    }

    // --- Ring: frame alignment ---

    #[test]
    fn pop_never_splits_frames() {
        let ring = Ring::new(&layout(4), 60, 120);
        ring.push(&counter_bytes(0..10)); // 2.5 frames
        let mut out = [0u8; 64];
        // Only the two whole frames come out; the half frame stays buffered.
        assert_eq!(ring.pop_into(&mut out), 8);
        assert_eq!(out[..8], counter_bytes(0..8)[..]);
        // Completing the frame makes it poppable.
        ring.push(&counter_bytes(10..12));
        assert_eq!(ring.pop_into(&mut out), 4);
        assert_eq!(out[..4], counter_bytes(8..12)[..]);
    }

    #[test]
    fn pop_into_odd_sized_output_fills_whole_frames_only() {
        let ring = Ring::new(&layout(4), 60, 120);
        ring.push(&counter_bytes(0..16));
        let mut out = [0xAA; 7]; // room for one frame plus change
        assert_eq!(ring.pop_into(&mut out), 4);
        assert_eq!(out[..4], counter_bytes(0..4)[..]);
        assert_eq!(out[4..], [0xAA; 3]); // the odd tail is not written
    }

    #[test]
    fn block_align_one_moves_arbitrary_byte_counts() {
        let ring = Ring::new(&layout(1), 8, 16);
        ring.push(&counter_bytes(0..5));
        let mut out = [0u8; 3];
        assert_eq!(ring.pop_into(&mut out), 3);
        assert_eq!(out, [0, 1, 2]);
        let mut rest = [0u8; 8];
        assert_eq!(ring.pop_into(&mut rest), 2);
        assert_eq!(rest[..2], [3, 4]);
    }

    // --- Ring: overflow clamping ---

    #[test]
    fn push_at_exactly_max_occupancy_does_not_drop() {
        let ring = Ring::new(&layout(4), 8, 16); // target 8, max 16 bytes
        ring.push(&counter_bytes(0..16));
        let mut out = [0u8; 32];
        assert_eq!(ring.pop_into(&mut out), 16);
        assert_eq!(out[..16], counter_bytes(0..16)[..]);
    }

    #[test]
    fn overflow_drops_oldest_bytes_back_to_target() {
        let ring = Ring::new(&layout(4), 8, 16); // target 8, max 16 bytes
        ring.push(&counter_bytes(0..20)); // 4 bytes over max
        let mut out = [0u8; 32];
        // Clamped back to the 8-byte target, keeping the newest bytes.
        assert_eq!(ring.pop_into(&mut out), 8);
        assert_eq!(out[..8], counter_bytes(12..20)[..]);
    }

    #[test]
    fn overflow_clamp_drops_whole_frames_only() {
        let ring = Ring::new(&layout(4), 8, 16);
        ring.push(&counter_bytes(0..18)); // 4.5 frames; 10 bytes over target
        // Only two whole frames (8 bytes) are dropped, not 10 raw bytes.
        let mut out = [0u8; 32];
        assert_eq!(ring.pop_into(&mut out), 8);
        assert_eq!(out[..8], counter_bytes(8..16)[..]);
        // The trailing half frame survived the clamp awaiting completion.
        ring.push(&counter_bytes(18..20));
        assert_eq!(ring.pop_into(&mut out), 4);
        assert_eq!(out[..4], counter_bytes(16..20)[..]);
    }

    #[test]
    fn repeated_pushes_clamp_once_max_is_crossed() {
        let ring = Ring::new(&layout(4), 8, 16);
        for chunk in counter_bytes(0..24).chunks(4) {
            ring.push(chunk);
        }
        // Crossing 16 bytes at the 20-byte mark clamps to bytes 12..20;
        // the final frame lands after the clamp.
        let mut out = [0u8; 32];
        assert_eq!(ring.pop_into(&mut out), 12);
        assert_eq!(out[..12], counter_bytes(12..24)[..]);
    }

    // --- Ring: wrap-around (non-contiguous VecDeque) ---

    #[test]
    fn pop_reassembles_frames_split_across_the_deque_seam() {
        let ring = Ring::new(&layout(4), 60, 120);
        // A fully drained VecDeque snaps its head back to the start of its
        // allocation, so keep one frame resident while interleaved push/pop
        // cycles walk the head towards the physical end of the (never
        // reallocated) buffer, stopping within two frames of it.
        let cap = ring.lock().capacity();
        let mut next_in: u8 = 0;
        let mut next_out: u8 = 0;
        ring.push(&feed(&mut next_in, 4));
        let mut tmp = [0u8; 4];
        for _ in 0..(cap - 8) / 4 {
            ring.push(&feed(&mut next_in, 4));
            assert_eq!(ring.pop_into(&mut tmp), 4);
            expect(&mut next_out, &tmp);
        }
        // Three more frames push the resident data across the seam.
        ring.push(&feed(&mut next_in, 12));
        assert!(
            !ring.lock().as_slices().1.is_empty(),
            "setup failed to wrap the deque"
        );
        let mut out = [0u8; 16];
        assert_eq!(ring.pop_into(&mut out), 16);
        expect(&mut next_out, &out);
    }

    #[test]
    fn interleaved_push_pop_preserves_fifo_order_across_wraps() {
        let ring = Ring::new(&layout(4), 60, 120);
        let cap = ring.lock().capacity();
        let mut next_in: u8 = 0;
        let mut next_out: u8 = 0;
        let mut pushed = 0;
        let mut wrapped = false;
        // The resident frame keeps the deque from emptying (which would
        // reset its head and prevent wrap-around, see the test above).
        ring.push(&feed(&mut next_in, 4));
        while pushed < cap * 12 {
            ring.push(&feed(&mut next_in, 12));
            pushed += 12;
            wrapped |= !ring.lock().as_slices().1.is_empty();
            let mut out = [0u8; 12];
            assert_eq!(ring.pop_into(&mut out), 12);
            expect(&mut next_out, &out);
        }
        assert!(wrapped, "ring never wrapped; the test proved nothing");
    }
}
