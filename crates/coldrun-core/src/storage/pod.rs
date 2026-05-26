//! POD column storage: `Vec` while building, `Arc<[T]>` after read from disk.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use crate::Result;

#[derive(Debug, Clone)]
pub enum PodStorage<T: Copy> {
    Owned(Vec<T>),
    Shared(Arc<[T]>),
}

impl<T: Copy> PodStorage<T> {
    pub fn owned_with_capacity(n: usize) -> Self {
        Self::Owned(Vec::with_capacity(n))
    }

    pub fn push(&mut self, v: T) -> Result<()> {
        match self {
            Self::Owned(vec) => {
                vec.push(v);
                Ok(())
            }
            Self::Shared(_) => Err(crate::Error::msg("cannot push into shared column buffer")),
        }
    }

    pub fn extend_from_slice(&mut self, other: &[T]) -> Result<()> {
        match self {
            Self::Owned(vec) => {
                vec.extend_from_slice(other);
                Ok(())
            }
            Self::Shared(_) => Err(crate::Error::msg("cannot extend shared column buffer")),
        }
    }

    pub fn from_vec(vec: Vec<T>) -> Self {
        Self::Owned(vec)
    }

    pub fn from_arc(arc: Arc<[T]>) -> Self {
        Self::Shared(arc)
    }
}

impl<T: Copy> Deref for PodStorage<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        match self {
            Self::Owned(v) => v,
            Self::Shared(a) => a,
        }
    }
}

impl<T: Copy> DerefMut for PodStorage<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        match self {
            Self::Owned(v) => v,
            Self::Shared(_) => panic!("cannot mutably dereference shared column buffer"),
        }
    }
}
