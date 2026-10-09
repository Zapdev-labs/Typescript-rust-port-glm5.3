use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
struct TestProgram {
    options: CompilerOptions,
    source_file: SourceFile,
    bind: BindResult,
    binder_symbol_count: u32,
    checker_symbols: Vec<Symbol>,
}
impl Program for TestProgram {
    fn options(&self) -> &CompilerOptions { &self.options }
    fn source_files(&self) -> &[SourceFile] { std::slice::from_ref(&self.source_file) }
    fn bind_source_files(&mut self) {}
    fn node_store(&self) -> &dyn NodeStore { &self.source_file }
    fn symbol(&self, id: SymbolId) -> &Symbol {
        if id.0 < self.binder_symbol_count { &self.bind.symbols[id.0 as usize] }
        else { &self.checker_symbols[(id.0 - self.binder_symbol_count) as usize] }
    }
    fn symbol_mut(&mut self, id: SymbolId) -> &mut Symbol {
        if id.0 < self.binder_symbol_count { &mut self.bind.symbols[id.0 as usize] }
        else { &mut self.checker_symbols[(id.0 - self.binder_symbol_count) as usize] }
    }
    fn alloc_symbol(&mut self, symbol: Symbol) -> SymbolId {
        self.checker_symbols.push(symbol);
        SymbolId(self.binder_symbol_count + self.checker_symbols.len() as u32 - 1)
    }
}
fn main() {
    for src in [
        "interface I { [a: string]: number; [b: string]: string; }",
        "class C { constructor() {} constructor() {} }",
        "enum E { A = 1, A = 2 }",
        "module M { module M { } }",
    ] {
        let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
        let file = sf.as_node_id();
        let bind = bind_source_file(&mut sf).expect("bind");
        let binder_symbol_count = bind.symbol_count as u32;
        let program = TestProgram { options: CompilerOptions::default(), source_file: sf, bind, binder_symbol_count, checker_symbols: Vec::new() };
        let mut checker = new_checker(Box::new(program));
        checker.check_source_file(file, true);
        println!("SRC: {}\n  DIAGS: {:?}", src, checker.get_file_diagnostics(file).iter().map(|d| (d.code(), d.message_text())).collect::<Vec<_>>());
    }
}
