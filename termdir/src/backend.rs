//! Backend choice is explicit at launch; native frames are never inferred in the guest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Backend {
    #[default]
    Terminal,
    Ui4,
}

impl Backend {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "terminal" => Ok(Self::Terminal),
            "ui4" => Ok(Self::Ui4),
            _ => Err(format!(
                "termdir: unknown backend {value:?}; expected ui4 or terminal"
            )),
        }
    }
}
