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

/// Retoma o gerador com um valor e um modo (next/throw/return).
pub fn resume(vm: &mut Vm, this: &Value, v: Value, mode: u8) -> Result<Value, Value> {
    let Value::Object(g) = this else {
        let d = vm.describe(this);
        return Err(vm.type_error(format!("next method called on incompatible receiver {d}")));
    };
    let (state, frame) = {
        let mut b = g.borrow_mut();
        match &mut b.kind {
            Kind::Generator(d) => {
                let s = d.state;
                if s == GenState::Running {
                    (s, None)
                } else {
                    (s, d.frame.take())
                }
            }
            _ => {
                drop(b);
                let d = vm.describe(this);
                return Err(vm.type_error(format!("next method called on incompatible receiver {d}")));
            }
        }
    };
    match state {
        GenState::Running => return Err(vm.type_error("Generator is already running")),
        GenState::Done => {
            return match mode {
                RESUME_THROW => Err(v),
                RESUME_RETURN => Ok(iter_result(vm, v, true)),
                _ => Ok(iter_result(vm, Value::Undefined, true)),
            };
        }
        GenState::Start if mode != RESUME_NEXT => {
            set_state(g, GenState::Done, None);
            return if mode == RESUME_THROW { Err(v) } else { Ok(iter_result(vm, v, true)) };
        }
        _ => {}
    }
    let Some(mut frame) = frame else {
        set_state(g, GenState::Done, None);
        return Ok(iter_result(vm, Value::Undefined, true));
    };
    if state == GenState::Yield {
        frame.stack.push(v);
        frame.stack.push(Value::Number(mode as f64));
    }
    set_state(g, GenState::Running, None);
    frame.suspended = false;
    let r = vm.run_frame(&mut frame);
    match r {
        Ok(v) if frame.suspended => {
            set_state(g, GenState::Yield, Some(frame));
            Ok(iter_result(vm, v, false))
        }
        Ok(v) => {
            set_state(g, GenState::Done, None);
            Ok(iter_result(vm, v, true))
        }
        Err(e) => {
            set_state(g, GenState::Done, None);
            Err(e)
        }
    }
}
