use crate::{IntoLua, LuaState};

/// Any number of Lua values.
///
/// As the last parameter of a typed callback it collects every remaining
/// argument (`|first: String, rest: Variadic<Value>|`); as a return value it
/// pushes all of its elements.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Variadic<T>(pub Vec<T>);

impl<T> std::ops::Deref for Variadic<T> {
    type Target = Vec<T>;

    #[inline]
    fn deref(&self) -> &Vec<T> {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Variadic<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.0
    }
}

impl<T> From<Vec<T>> for Variadic<T> {
    fn from(values: Vec<T>) -> Self {
        Variadic(values)
    }
}

impl<T> IntoIterator for Variadic<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<T: IntoLua> IntoLua for Variadic<T> {
    fn into_lua(self, state: &mut LuaState) -> Result<usize, String> {
        self.0.into_lua(state)
    }
}
