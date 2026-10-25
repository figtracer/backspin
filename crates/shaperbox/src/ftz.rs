//! Flush-to-zero for the ShaperBox section: processBlock sets the FPCR FZ bit before it runs the
//! graph (core.md section 2.3), so denormal results become zero. The port does the same while it
//! runs the section and restores the previous mode afterwards.

/// Sets FZ on creation and restores the previous FPCR on drop.
pub struct FlushToZero {
    #[cfg(target_arch = "aarch64")]
    saved: u64,
}

impl FlushToZero {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            let saved: u64;
            // SAFETY: reads and writes the floating-point control register of this thread only.
            unsafe {
                std::arch::asm!("mrs {}, fpcr", out(reg) saved);
                std::arch::asm!("msr fpcr, {}", in(reg) saved | (1 << 24));
            }
            Self { saved }
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            Self {}
        }
    }
}

impl Drop for FlushToZero {
    fn drop(&mut self) {
        #[cfg(target_arch = "aarch64")]
        // SAFETY: restores the value read in `new`.
        unsafe {
            std::arch::asm!("msr fpcr, {}", in(reg) self.saved);
        }
    }
}
