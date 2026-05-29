use std::{
    borrow::Cow,
    cell::{BorrowError, Ref, RefCell, RefMut},
    ops::Deref,
    sync::{Arc, Mutex},
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

    pub fn borrow<'s>(&'s self) -> Ref<'s, Inner> {
        self.inner.borrow()
    }

    pub fn borrow_mut<'s>(&'s self) -> RefMut<'s, Inner> {
        self.inner.borrow_mut()
    }

    pub unsafe fn borrow_unguraded(&self) -> Result<&Inner, BorrowError> {
        unsafe { self.inner.try_borrow_unguarded() }
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
        unsafe { self.inner.try_borrow_unguarded() }
            .unwrap()
            .messages()
    }
}

unsafe impl<Inner> Send for OllamaSharedChatHistory<Inner> where Inner: Send {}
