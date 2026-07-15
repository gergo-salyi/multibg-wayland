const SSSE3     : u32 = 1 << 0;
const AVX2      : u32 = 1 << 1;
const AVX512BW  : u32 = 1 << 2;
const AVX512VBMI: u32 = 1 << 3;

#[derive(Clone, Copy)]
pub struct CpuFeatures(u32);

impl CpuFeatures {
    pub fn detect() -> Self {
        let mut detected = 0;
        if is_x86_feature_detected!("ssse3") {
            detected |= SSSE3;
        }
        if is_x86_feature_detected!("avx")
            && is_x86_feature_detected!("avx2")
        {
            detected |= AVX2;
        }
        if is_x86_feature_detected!("avx512f")
            && is_x86_feature_detected!("avx512bw")
        {
            detected |= AVX512BW;
        }
        if detected & AVX512BW != 0
            && is_x86_feature_detected!("avx512vbmi")
        {
            detected |= AVX512VBMI;
        }

        #[cfg(debug_assertions)] {
            let requested = match std::env::var("MULTIBG_DEBUG_CPU").as_deref()
            {
                Ok("baseline") => 0,
                Ok("ssse3") => SSSE3,
                Ok("avx2") => SSSE3 | AVX2,
                Ok("avx512bw") => SSSE3 | AVX2 | AVX512BW,
                Ok("avx512vbmi") => SSSE3 | AVX2 | AVX512BW | AVX512VBMI,
                Ok(_) => panic!(),
                Err(_) => SSSE3 | AVX2 | AVX512BW | AVX512VBMI,
            };
            assert!(requested & !detected == 0);
            detected = requested;
        }

        Self(detected)
    }

    pub fn ssse3(&self) -> bool {
        self.0 & SSSE3 != 0
    }

    pub fn avx2(&self) -> bool {
        self.0 & AVX2 != 0
    }

    pub fn avx512bw(&self) -> bool {
        self.0 & AVX512BW != 0
    }

    pub fn avx512vbmi(&self) -> bool {
        self.0 & AVX512VBMI != 0
    }
}
