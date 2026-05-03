use crate::{agent::memory::dialog::DialogMemory, error::NaE};

pub struct DebugDialogMemory<D> {
    mem: Option<D>,
}

impl<D> DebugDialogMemory<D> {
    pub fn new() -> Self {
        Self { mem: None }
    }
}

impl<D> Default for DebugDialogMemory<D> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D> DialogMemory for DebugDialogMemory<D>
where
    D: Clone + Send + Sync,
{
    type Dialog = D;
    type Error = NaE;

    async fn update(&mut self, dialog: &Self::Dialog) -> Result<(), Self::Error> {
        self.mem = Some(dialog.clone());
        Ok(())
    }

    async fn get(&self) -> Result<Option<Self::Dialog>, Self::Error> {
        Ok(self.mem.clone())
    }
}
