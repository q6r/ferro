fn main() {
    let src = std::env::args().nth(1).unwrap_or("for (let i = 0; i < 3e6; i++) {}".into());
    let prog = ferro_js::parser::parse_program(&src).unwrap();
    let code = ferro_js::compiler::compile_script(&prog, "x").unwrap();
    for (i, op) in code.ops.iter().enumerate() {
        println!("{i:3} {op:?}");
    }
    println!("nregs={} var_end={} size_of<Op>={} size_of<Value>={}", code.nregs, code.var_end, std::mem::size_of::<ferro_js::compiler::Op>(), std::mem::size_of::<ferro_js::Value>());
}
