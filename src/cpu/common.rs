#[derive(Clone, Copy)]
pub struct CpuFeatures;

impl CpuFeatures {
    pub fn detect() -> Self {
        Self
    }
}
