
use llama_runner::error::RunnerError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConditionMatcherError {
    #[error("llama: {0}")]
    Llama(RunnerError),
    #[error("template: {0}")]
    TemplateExpansion(#[from] TemplateExpansionError),
}

#[derive(Debug, Error)]
pub enum TemplateExpansionError {
    #[error("xml parsing: {0}")]
    XmlParsing(#[from] quick_xml::Error),
    #[error("invalid tag: {0}")]
    InvalidTag(String),
    #[error("xml structure is invalid")]
    InvalidHirarchy,
    #[error("invalid macro: {0}")]
    InvalidMacro(String),
}

#[derive(Debug, Error)]
pub enum GetTruthValueError<Model, Dec, Crit, Runner> {
    #[error("agent model: {0}")]
    Model(Model),
    #[error("agent runner: {0}")]
    Runner(Runner),
    #[error("decision memory: {0}")]
    DecisionMemory(Dec),
    #[error("criteria memory: {0}")]
    CriteriaMemory(Crit),
}
