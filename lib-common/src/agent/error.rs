use thiserror::Error;

#[derive(Debug, Error)]
pub enum TemplateExpansionError<Macro> {
    #[error("xml parsing: {0}")]
    XmlParsing(#[from] quick_xml::Error),
    #[error("invalid tag: {0}")]
    InvalidTag(String),
    #[error("xml structure is invalid")]
    InvalidHirarchy,
    #[error("invalid macro: {0}")]
    InvalidMacro(String),
    #[error("macro internal error: {0}")]
    MacroInternal(Macro),
}

#[derive(Debug, Error)]
pub enum GetTruthValueError<Dec, Crit, Runner> {
    #[error("agent runner: {0}")]
    Runner(Runner),
    #[error("decision memory: {0}")]
    DecisionMemory(Dec),
    #[error("criteria memory: {0}")]
    CriteriaMemory(Crit),
}
