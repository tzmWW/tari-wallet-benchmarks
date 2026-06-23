use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeName {
    OldWallet,
    NewWallet,
    PaymentProcessor,
}

impl ModeName {
    pub const ALL: [Self; 3] = [Self::OldWallet, Self::NewWallet, Self::PaymentProcessor];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OldWallet => "old_wallet",
            Self::NewWallet => "new_wallet",
            Self::PaymentProcessor => "payment_processor",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioName {
    B0,
    S0,
    S1,
    S2,
    S3,
    S4,
    S5,
    S6,
    S7,
}

impl ScenarioName {
    pub const ALL: [Self; 9] = [
        Self::B0,
        Self::S0,
        Self::S1,
        Self::S2,
        Self::S3,
        Self::S4,
        Self::S5,
        Self::S6,
        Self::S7,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::B0 => "B0",
            Self::S0 => "S0",
            Self::S1 => "S1",
            Self::S2 => "S2",
            Self::S3 => "S3",
            Self::S4 => "S4",
            Self::S5 => "S5",
            Self::S6 => "S6",
            Self::S7 => "S7",
        }
    }

    pub fn measurement_surface(self, mode: ModeName) -> &'static str {
        match (mode, self) {
            (ModeName::PaymentProcessor, Self::B0 | Self::S2 | Self::S3 | Self::S6 | Self::S7) => {
                "companion_wallet_scan"
            }
            (ModeName::PaymentProcessor, _) => "payment_processor_api",
            (ModeName::OldWallet, _) => "console_wallet_grpc",
            (ModeName::NewWallet, _) => "minotari_library",
        }
    }
}
