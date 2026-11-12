//! Ferro JS: motor de JavaScript próprio, 100% Rust.
//!
//! Fluxo: texto → `lexer` → `parser` (AST) → `compiler` (bytecode) → `vm`.
//! Os objetos embutidos (Object, Array, JSON, console...) ficam em `builtins`.

pub mod ast;
pub mod bigint;
pub mod builtins;
pub mod compiler;
pub mod fxhash;
pub mod gc;
pub mod conv;
pub mod lexer;
pub mod module;
pub mod ops;
pub mod parser;
pub mod regex;
pub mod resolve;
pub mod string;
pub mod value;
pub mod vm;

pub use string::JsStr;
pub use value::{ObjRef, Value};
pub use vm::{Out, Vm};

/// Interface simples para quem embute o motor (o navegador, a linha de comando).
pub struct Engine {
    pub vm: Vm,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Engine {
        Engine { vm: Vm::new() }
    }

    /// Executa um script; devolve o valor da última expressão ou a exceção.
    /// No fim roda as microtarefas (promessas), como os navegadores fazem.
    pub fn eval(&mut self, src: &str, file: &str) -> Result<Value, Value> {
        let r = self.vm.eval_script(src, file);
        self.vm.run_microtasks();
        r
    }

    /// Roda timers e microtarefas até não sobrar nada (linha de comando).
    pub fn run_event_loop(&mut self) {
        self.vm.run_event_loop();
    }

    /// Roda só os timers que já venceram (REPL), sem esperar.
    pub fn run_due_timers(&mut self) {
        self.vm.run_microtasks();
        while self.vm.run_one_timer(false) {}
    }

    /// Para onde vai a saída do `console`.
    pub fn set_output(&mut self, f: impl FnMut(Out, &str) + 'static) {
        self.vm.output = Box::new(f);
    }

    /// Texto legível de um valor (como o REPL do Node mostra).
    pub fn inspect(&self, v: &Value) -> String {
        match v {
            Value::String(s) => builtins::console::inspect(&self.vm, &Value::String(s.clone()), 2)
                .to_string()
                .pipe_quote(),
            _ => builtins::console::inspect(&self.vm, v, 2),
        }
    }

    /// Mensagem de uma exceção não tratada ("Uncaught TypeError: ...").
    pub fn error_text(&self, e: &Value) -> String {
        let body = match e {
            Value::Object(o) if matches!(o.borrow().kind, value::Kind::Error) => {
                builtins::console::inspect(&self.vm, e, 2)
            }
            Value::String(_) => self.inspect(e),
            _ => builtins::console::inspect(&self.vm, e, 2),
        };
        format!("Uncaught {body}")
    }
}

trait PipeQuote {
    fn pipe_quote(self) -> String;
}

impl PipeQuote for String {
    /// Strings no REPL aparecem entre aspas.
    fn pipe_quote(self) -> String {
        let q = if !self.contains('\'') { '\'' } else if !self.contains('"') { '"' } else { '`' };
        let escaped = self.replace('\\', "\\\\").replace('\n', "\\n");
        format!("{q}{escaped}{q}")
    }
}

#[cfg(test)]
mod tests;
