#![warn(unused_must_use)]
#![warn(rust_2018_idioms)]
#![allow(clippy::redundant_closure)]
#![allow(clippy::vec_box)]
// #![feature(const_fn)]
#![feature(box_syntax)]
#![allow(dead_code)]
#![allow(unused_variables)]
#[macro_use]
pub mod macros;
pub mod ast2cpp;
pub mod err;
pub mod eval;
pub mod gccjit;
pub mod ir;
pub mod optimize;
pub mod semantic;
pub mod semck;
pub mod syntax;

pub use syntax::{ast, position::Position};

pub use syntax::interner::{intern, str, INTERNER};

use parking_lot::{Mutex, RwLock};
lazy_static::lazy_static! (
    pub static ref IDGEN: Mutex<RwLock<NodeIdGenerator>> = Mutex::new(RwLock::new(NodeIdGenerator::new()));
);

#[inline]
pub fn gen_id() -> NodeId {
    let lock = IDGEN.lock();
    let read = lock.read();

    read.next()
}

use std::cell::RefCell;
#[derive(Debug, Default)]
pub struct NodeIdGenerator {
    value: RefCell<usize>,
}

use syntax::ast::NodeId;

unsafe impl Sync for NodeIdGenerator {}
unsafe impl Send for NodeIdGenerator {}

impl NodeIdGenerator {
    pub fn new() -> NodeIdGenerator {
        NodeIdGenerator {
            value: RefCell::new(1),
        }
    }

    pub fn next(&self) -> NodeId {
        let value = *self.value.borrow();
        *self.value.borrow_mut() += 1;

        NodeId(value)
    }
}

use crate::syntax::ast::Function;
use ast::Type;
use std::collections::{HashMap, HashSet};
use syntax::ast::File;

/// Context stores ifnromation about program
pub struct Context {
    pub file: File,
    pub types: HashMap<NodeId, Type>,
    pub gced: HashSet<NodeId>,
    pub opt: u8,
    pub jit: bool,
    pub emit_asm: bool,
    pub emit_obj: bool,
    pub output: String,
    pub shared: bool,
    pub gimple: bool,
}

impl Context {
    pub fn new(file: File) -> Context {
        Context {
            file,
            types: HashMap::new(),
            gced: HashSet::new(),
            opt: 2,
            emit_asm: false,
            emit_obj: false,
            jit: true,
            output: String::new(),
            shared: false,
            gimple: false,
        }
    }

    pub fn import(&mut self, path: &str) {
        let import = if self.file.root.is_empty() {
            path.to_owned()
        } else {
            format!("{}/{}", self.file.root, path)
        };
        let mut file = File {
            elems: vec![],
            src: String::new(),
            path: String::new(),
            root: import.clone(),
        };
        use crate::syntax::{lexer, parser::Parser};
        use lexer::reader::Reader;
        use syntax::ast::Elem;
        let reader = Reader::from_file(&import).expect("File not found");
        let mut parser = Parser::new(reader, &mut file);
        parser.parse().expect("Error");

        let mut ctx = Context::new(file);
        ctx.imports();

        for elem in ctx.file.elems {
            match elem {
                Elem::Func(f) => {
                    if f.public && !f.static_ {
                        self.file.elems.push(Elem::Func(f.clone()));
                    }
                }
                Elem::Struct(s) => {
                    if s.public {
                        self.file.elems.push(Elem::Struct(s.clone()));
                    }
                }
                Elem::Const(s) => {
                    if s.public {
                        self.file.elems.push(Elem::Const(s.clone()));
                    }
                }
                Elem::Link(name) => self.file.elems.push(Elem::Link(name)),
                _ => (),
            }
        }
    }

    pub fn get_func_mut(&mut self, id: NodeId) -> Option<&mut Function> {
        for elem in self.file.elems.iter_mut() {
            if let syntax::ast::Elem::Func(f) = elem {
                return Some(f);
            }
        }
        None
    }

    pub fn imports(&mut self) {
        use syntax::ast::Elem;
        for elem in self.file.elems.clone().iter() {
            if let Elem::Import(path) = elem {
                self.import(&path);
            }
        }
    }
}
