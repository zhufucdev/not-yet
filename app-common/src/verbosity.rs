use clap_verbosity_flag::Verbosity;

pub trait WithVerbosity {
    fn get_verbosity(&self) -> Verbosity;
}
