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

    /// Roda um timer (o que vence primeiro). `wait`: espera ele vencer.
    /// Devolve false se não havia timer para rodar.
    pub fn run_one_timer(&mut self, wait: bool) -> bool {
        let Some(i) = (0..self.timers.len()).min_by(|&a, &b| {
            let (ta, tb) = (&self.timers[a], &self.timers[b]);
            ta.due.partial_cmp(&tb.due).unwrap_or(std::cmp::Ordering::Equal).then(ta.id.cmp(&tb.id))
        }) else {
            return false;
        };
        let now = self.now_ms();
        let due = self.timers[i].due;
        if due > now {
            if !wait {
                return false;
            }
            std::thread::sleep(Duration::from_secs_f64((due - now) / 1000.0));
        }
        let t = self.timers.remove(i);
        if let Some(iv) = t.interval {
            // reagenda antes de chamar: o callback pode cancelar o próprio intervalo
            let mut again = t.clone();
            again.due = (t.due + iv).max(self.now_ms());
            self.timers.push(again);
        }
        let r = match &t.callback {
            Value::String(src) => {
                let src = src.to_rust();
                self.eval_script(&src, "timer")
            }
            cb => self.call(cb, Value::Undefined, t.args.clone()),
        };
        if let Err(e) = r {
            self.report_uncaught(&e);
        }
        self.run_microtasks();
        true
    }

    /// Laço de eventos: microtarefas e timers até não sobrar nada.
    pub fn run_event_loop(&mut self) {
        self.run_microtasks();
        while self.run_one_timer(true) {
            if crate::gc::should_collect() {
                crate::gc::collect();
            }
        }
    }
}
