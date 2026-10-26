//! Flush-to-zero for the ShaperBox section: processBlock sets the FPCR FZ bit before it runs the
//! graph (core.md section 2.3), so denormal results become zero. The port does the same while it
//! runs the section and restores the previous mode afterwards. On x86-64, FTZ and DAZ in MXCSR
//! give the same behaviour (FZ also flushes denormal inputs).

/// Sets FZ on creation and restores the previous mode on drop.
pub struct FlushToZero {
    #[cfg(target_arch = "aarch64")]
    saved: u64,
    #[cfg(target_arch = "x86_64")]
    saved: u32,
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
        #[cfg(target_arch = "x86_64")]
        {
            let mut saved = 0u32;
            let set = |v: &u32| {
                // SAFETY: loads MXCSR of this thread from a valid u32.
                unsafe { std::arch::asm!("ldmxcsr [{}]", in(reg) v) }
            };
            // SAFETY: stores MXCSR of this thread into a valid u32.
            unsafe { std::arch::asm!("stmxcsr [{}]", in(reg) &mut saved) };
            // FTZ (bit 15) and DAZ (bit 6).
            set(&(saved | 1 << 15 | 1 << 6));
            Self { saved }
        }
        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
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
        #[cfg(target_arch = "x86_64")]
        // SAFETY: restores the value read in `new`.
        unsafe {
            std::arch::asm!("ldmxcsr [{}]", in(reg) &self.saved);
        }
    }
}
