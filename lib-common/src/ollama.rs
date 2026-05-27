use std::{
    borrow::Cow,
    cell::RefCell,
    ops::Deref,
    sync::{Arc, Mutex, RwLock},
};

use ollama_rs::history::ChatHistory;

#[derive(Debug, Default, Clone)]
pub struct OllamaSharedChatHistory<Inner> {
    inner: Arc<RefCell<Inner>>,
    write_lock: Arc<Mutex<()>>,
}

impl<Inner> OllamaSharedChatHistory<Inner> {
    pub fn new(inner: Inner) -> Self {
        Self {
            inner: Arc::new(RefCell::new(inner)),
            write_lock: Arc::new(Mutex::new(())),
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
        let guard = self.write_lock.lock().unwrap();
        self.inner.borrow_mut().push(message);
        drop(guard);
    }

    fn messages(&self) -> Cow<'_, [ollama_rs::generation::chat::ChatMessage]> {
        self.deref().messages()
    }
}

unsafe impl<Inner> Send for OllamaSharedChatHistory<Inner> where Inner: Send {}
