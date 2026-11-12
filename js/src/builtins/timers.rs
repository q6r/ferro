//! setTimeout/setInterval e o laço de eventos da linha de comando.
//! (No navegador, o laço de eventos da aba usa a mesma fila de timers.)

use std::time::Duration;

use super::Realm;
use crate::value::*;
use crate::vm::Vm;

#[derive(Clone)]
pub struct Timer {
    pub id: u32,
    /// Quando dispara (ms desde o início da VM).
    pub due: f64,
    pub callback: Value,
    pub args: Vec<Value>,
    pub interval: Option<f64>,
}

pub fn install(r: &Realm) {
    let g = &r.global;
    r.method(g, "setTimeout", 2, set_timeout);
    r.method(g, "setInterval", 2, set_interval);
    r.method(g, "clearTimeout", 1, clear_timer);
    r.method(g, "clearInterval", 1, clear_timer);
}

fn add_timer(vm: &mut Vm, a: &CallArgs, repeat: bool) -> Result<Value, Value> {
    let delay = if a.arg(1).is_undefined() { 0.0 } else { vm.to_number(&a.arg(1))? };
    let delay = if delay.is_finite() && delay > 0.0 { delay } else { 0.0 };
    vm.timer_seq += 1;
    let id = vm.timer_seq;
    let now = vm.now_ms();
    vm.timers.push(Timer {
        id,
        due: now + delay,
        callback: a.arg(0),
        args: a.args.iter().skip(2).cloned().collect(),
        interval: if repeat { Some(delay.max(1.0)) } else { None },
    });
    Ok(Value::Number(id as f64))
}

fn set_timeout(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_timer(vm, a, false)
}

fn set_interval(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    add_timer(vm, a, true)
}

fn clear_timer(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Value::Number(n) = a.arg(0) {
        let id = n as u32;
        vm.timers.retain(|t| t.id != id);
    }
    Ok(Value::Undefined)
}

impl Vm {
    pub fn now_ms(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64() * 1000.0
    }

