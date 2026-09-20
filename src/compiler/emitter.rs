mod lua53;
pub use lua53::Lua53;

use super::utils::DFS;
use crate::parser::ast::*;
use crate::parser::parser::*;

pub trait Emitter: DFS<()> {
    fn write(&self, ast: &Tree<NodeSyntax>, module_name: &str, target_path: &str);
}

pub struct EmitDriver {
    emitters: Vec<Box<dyn Emitter>>,
}

impl EmitDriver {
    pub fn new() -> Self {
        EmitDriver { emitters: vec![] }
    }

    pub fn add_emitter(&mut self, emitter: Box<dyn Emitter>) {
        self.emitters.push(emitter);
    }

    pub fn write(&self, ast: &Tree<NodeSyntax>, module_name: &str, target_path: &str) {
        for emitter in &self.emitters {
            emitter.write(ast, module_name, target_path);
        }
    }
}
