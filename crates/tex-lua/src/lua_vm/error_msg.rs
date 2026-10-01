use crate::LuaValue;

#[derive(Debug, Default)]
pub enum ErrorMsg {
    #[default]
    None,
    Msg(String),
    Object(LuaValue),
    /// An error that escaped a top-level host call, rendered while its frames
    /// still existed (lua.c's `msghandler`): `message` alone and `full` with
    /// the stack traceback.
    Traced { message: String, full: String },
}
