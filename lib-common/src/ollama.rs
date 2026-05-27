use std::{
    borrow::Cow, cell::RefCell, ops::Deref, sync::{Arc, Mutex, RwLock}
};

use ollama_rs::history::ChatHistory;

#[derive(Debug, Default)]
pub struct OllamaSharedChatHistory<Inner> {
    inner: RefCell<Inner>,
    write_lock: Mutex<()>
}

impl<Inner> OllamaSharedChatHistory<Inner> {
    pub fn new(inner: Inner) -> Self {
        Self {
            inner: RefCell::new(inner),
            write_lock: Mutex::new(())
        }
    }
}

impl<Inner> Deref for OllamaSharedChatHistory<Inner> {
    type Target = Inner;

    fn deref(&self) -> &Self::Target {
        unsafe { self.inner.try_borrow_unguarded().unwrap() }
    }
}

impl<Inner> ChatHistory for OllamaSharedChatHistory<Inner>
where
    Inner: ChatHistory,
{
    fn push(&mut self, message: ollama_rs::generation::chat::ChatMessage) {
        self.inner.borrow_mut().push(message);
    }

    fn messages(&self) -> Cow<'_, [ollama_rs::generation::chat::ChatMessage]> {
        self.deref().messages()
    }
}


