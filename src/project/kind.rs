use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectKind {
    Maven,
    Gradle,
    Unknown,
}

impl fmt::Display for ProjectKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Maven => "Maven",
            Self::Gradle => "Gradle",
            Self::Unknown => "Unknown",
        })
    }
}
