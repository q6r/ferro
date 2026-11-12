//! Geradores: `function*`, `yield`, `yield*` e os métodos next/return/throw.

use super::Realm;
use super::array::iter_result;
use super::promise::{RESUME_NEXT, RESUME_RETURN, RESUME_THROW};
use crate::value::*;
use crate::vm::{Frame, Vm};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GenState {
    /// Criado, ainda não começou.
    Start,
    /// Parado num `yield`.
    Yield,
    Running,
    Done,
}

pub struct GenData {
    pub frame: Option<Box<Frame>>,
    pub state: GenState,
}

pub fn install(r: &Realm) {
    let gp = &r.generator_proto;
    r.method(gp, "next", 1, next);
    r.method(gp, "return", 1, ret);
    r.method(gp, "throw", 1, throw);
    r.tag(gp, "Generator");
}

fn next(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    resume(vm, &a.this, a.arg(0), RESUME_NEXT)
}

fn ret(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    resume(vm, &a.this, a.arg(0), RESUME_RETURN)
}

fn throw(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    resume(vm, &a.this, a.arg(0), RESUME_THROW)
}

fn set_state(g: &ObjRef, state: GenState, frame: Option<Box<Frame>>) {
    if let Kind::Generator(d) = &mut g.borrow_mut().kind {
        d.state = state;
        d.frame = frame;
    }
}

