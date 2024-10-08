#[derive(Debug, Clone, Copy)]
pub enum Pan {
    /// Value ranges from 0 to 64, with 32 being center
    Value(u8),
    Surround,
    Disabled,
}

impl Default for Pan {
    fn default() -> Self {
        Self::Value(32)
    }
}

impl TryFrom<u8> for Pan {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            100 => Ok(Self::Surround),
            128 => Ok(Self::Disabled),
            0..=64 => Ok(Self::Value(value)),
            _ => Err(value),
        }
    }
}
