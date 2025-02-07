use crate::{
    intern,
    syntax::{
        lexer::token::{FloatSuffix, IntBase, IntSuffix},
        position::Position,
    },
};

pub fn rc<T>(v: T) -> Rc<RefCell<T>> {
    Rc::new(RefCell::new(v))
}

use std::{cell::RefCell, collections::HashMap, rc::Rc};
/// Constant value that known at compile-time
#[derive(Clone, PartialOrd, Debug)]
pub enum Const {
    /// Immediate value or just int value
    Imm(i64, IntSuffix, IntBase),
    /// Float value
    Float(f64, FloatSuffix),
    /// Boolean value
    Bool(bool),
    /// Struct value with fields
    Struct(Name, Vec<(Name, Rc<RefCell<Const>>, NodeId)>),
    /// Just a void value
    Void,
    /// Return value
    Ret(Rc<RefCell<Const>>),
    Str(String),
    Array(Rc<RefCell<Vec<Rc<RefCell<Const>>>>>),
    /// If evaluator seen this value then evaluation stops
    None,
}

impl Const {
    fn is_void(&self) -> bool {
        match self {
            Const::Void => true,
            _ => false,
        }
    }

    /// Translate Const value into Expression
    fn to_kind(&self) -> ExprKind {
        match self {
            Const::Imm(imm, suffix, base) => ExprKind::Int(*imm, base.clone(), suffix.clone()),
            Const::Float(f, suffix) => ExprKind::Float(*f, suffix.clone()),
            Const::Bool(b) => ExprKind::Bool(*b),
            Const::Struct(name, fields) => {
                let mut args = vec![];
                for (name, constant, id) in fields.iter() {
                    args.push(StructArg {
                        id: *id,
                        pos: Position::new(intern(""), 0, 0),
                        name: *name,
                        expr: box Expr {
                            id: NodeId(0),
                            pos: Position::new(intern(""), 0, 0),
                            kind: constant.borrow().to_kind(),
                        },
                    })
                }
                ExprKind::Struct(Path::new(*name), args)
            }
            Const::Ret(c) => c.borrow().to_kind(),
            Const::Str(s) => ExprKind::Str(s.to_owned()),
            v => panic!("{:?}", v),
        }
    }
}

use std::cmp::PartialEq;

impl PartialEq for Const {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Const::Imm(i, _, _), Const::Imm(i2, _, _)) => i == i2,
            (Const::Imm(i, _, _), Const::Float(f, _)) => *i as f64 == *f,
            (Const::Float(f, _), Const::Imm(i, _, _)) => *f == *i as f64,
            (Const::Float(f, _), Const::Float(f2, _)) => f == f2,
            (Const::Bool(b), Const::Bool(b2)) => b == b2,
            (Const::Struct(s1name, fields1), Const::Struct(s2name, fields2)) => {
                if fields1.len() == 0 && fields2.len() == 0 {
                    return s1name == s2name;
                } else {
                    let mut fields_ok = false;
                    for (f1, f2) in fields1.iter().zip(fields2.iter()) {
                        fields_ok = f1 == f2;
                    }
                    fields_ok
                }
            }
            _ => false,
        }
    }
}

impl Const {
    fn is_none(&self) -> bool {
        match self {
            Const::None => true,
            _ => false,
        }
    }
}
/// return size of type
fn ty_size(ty: &Type) -> Option<usize> {
    match ty {
        Type::Vector(_) => None, // TODO
        Type::Ptr(_) => Some(std::mem::size_of::<*const u8>()),
        Type::Basic(basic) => {
            let name: &str = &str(basic.name).to_string();
            match name {
                "u8" => Some(1),
                "u16" => Some(2),
                "u32" => Some(4),
                "u64" => Some(8),
                "usize" => Some(std::mem::size_of::<usize>()),
                "isize" => Some(std::mem::size_of::<isize>()),
                "i8" => Some(1),
                "i16" => Some(2),
                "i32" => Some(4),
                "i64" => Some(8),
                "char" => Some(1),
                _ => None,
            }
        }
        Type::Struct(s) => {
            let mut size = 0;
            for field in s.fields.iter() {
                if let Some(s) = ty_size(&field.data_type) {
                    size += s;
                } else {
                    return None;
                }
            }

            Some(size)
        }
        Type::Array(array) => {
            if array.len.is_some() {
                if let Some(size) = ty_size(&array.subtype) {
                    return Some(size * array.len.unwrap());
                } else {
                    return None;
                }
            } else {
                return Some(std::mem::size_of::<*const u8>());
            }
        }
        Type::Func(_) => return Some(std::mem::size_of::<*const u8>()),
        Type::Void(_) => return Some(0),
    }
}

use crate::{
    ast::*,
    syntax::interner::{str, Name},
    Context,
};
use std::intrinsics::transmute;

/// Constant evaluator that tries to evaluate code.
/// If `try_eval_normal` enabled then normal (non-constexpr) function evaluated
/// if possible too
pub struct ConstEval<'a> {
    /// Variables defined and known in compile-time context
    known_vars: HashMap<Name, Rc<RefCell<Const>>>,
    ctx: &'a mut Context,
    /// All constant functions
    const_functions: HashMap<Name, Vec<Function>>,
    return_: Option<Const>,
    constexprs: HashMap<Name, Expr>,
    functions: HashMap<Name, Vec<Function>>,
    try_eval_normal: bool,
    builtins: HashMap<Name, *const u8>,
    id: usize,
    running: bool,
    normal: bool,
}

impl<'a> ConstEval<'a> {
    /// Create new constant evaluator
    pub fn new(ctx: &'a mut Context, try_eval_normal: bool) -> ConstEval<'a> {
        ConstEval {
            ctx,
            known_vars: HashMap::new(),
            const_functions: HashMap::new(),
            return_: None,
            constexprs: HashMap::new(),
            functions: HashMap::new(),
            builtins: super::builtins::builtins(),
            try_eval_normal,
            id: 0,
            running: false,
            normal: false,
        }
    }
    /// try to get variable
    fn try_get_var(&mut self, name: &Name) -> Rc<RefCell<Const>> {
        if self.constexprs.contains_key(name) {
            let cexpr = self.constexprs.get(name).unwrap().clone();
            let val = self.eval(&cexpr);
            return val;
        }
        let var = self.known_vars.get(name);
        if var.is_none() {
            return Rc::new(RefCell::new(Const::None));
        }
        var.unwrap().clone()
    }
    /// If values of lhs and rhs known at compile time evaluates binary
    /// operation
    fn eval_binop(&mut self, op: &str, lhs: &Expr, rhs: &Expr) -> Rc<RefCell<Const>> {
        let c1 = self.eval(&lhs);
        let c2 = self.eval(&rhs);

        if c1.borrow().is_none() || c2.borrow().is_none() {
            return Rc::new(RefCell::new(Const::None));
        }
        let c1: &Const = &c1.borrow();
        let c2: &Const = &c2.borrow();
        let val = match op {
            "+" => match (c1.clone(), c2.clone()) {
                (Const::Imm(i1, suffix, base), Const::Imm(i2, _, _)) => {
                    Const::Imm(i1.overflowing_add(i2).0, suffix, base)
                }
                (Const::Float(f1, s), Const::Float(f2, _)) => Const::Float(f1 + f2, s),
                _ => Const::None,
            },
            "-" => match (c1, c2) {
                (Const::Imm(i1, s, b), Const::Imm(i2, _, _)) => {
                    Const::Imm(i1.overflowing_sub(*i2).0, *s, *b)
                }
                (Const::Float(f1, s), Const::Float(f2, _)) => Const::Float(f1 - f2, *s),
                _ => Const::None,
            },
            "/" => match (c1, c2) {
                (Const::Imm(i1, s, b), Const::Imm(i2, _, _)) => {
                    Const::Imm(i1.overflowing_div(*i2).0, *s, *b)
                }
                (Const::Float(f1, s), Const::Float(f2, _)) => Const::Float(f1 / f2, *s),
                _ => Const::None,
            },
            "*" => match (c1, c2) {
                (Const::Imm(i1, s, b), Const::Imm(i2, _, _)) => {
                    Const::Imm(i1.overflowing_mul(*i2).0, *s, *b)
                }
                (Const::Float(f1, s), Const::Float(f2, _)) => Const::Float(f1 * f2, *s),
                _ => Const::None,
            },
            "%" => match (c1, c2) {
                (Const::Imm(i1, s, b), Const::Imm(i2, _, _)) => Const::Imm(i1 % i2, *s, *b),
                (Const::Float(f1, s), Const::Float(f2, _)) => Const::Float(f1 % f2, *s),
                _ => Const::None,
            },
            "|" => match (c1, c2) {
                (Const::Imm(i1, s, b), Const::Imm(i2, _, _)) => Const::Imm(i1 | i2, *s, *b),
                _ => Const::None,
            },
            "&" => match (c1, c2) {
                (Const::Imm(i1, s, b), Const::Imm(i2, _, _)) => Const::Imm(i1 & i2, *s, *b),
                _ => Const::None,
            },
            ">>" => match (c1, c2) {
                (Const::Imm(i1, s, b), Const::Imm(i2, _, _)) => Const::Imm(i1 >> i2, *s, *b),
                _ => Const::None,
            },
            "<<" => match (c1, c2) {
                (Const::Imm(i1, s, b), Const::Imm(i2, _, _)) => Const::Imm(i1 << i2, *s, *b),
                _ => Const::None,
            },
            /*"==" => Const::Bool(c1 == c2),
            "!=" => Const::Bool(c1 != c2),
            ">" => Const::Bool(c1 > c2),
            "<" => Const::Bool(c1 < c2),
            ">=" => Const::Bool(c1 >= c2),
            "<=" => Const::Bool(c1 <= c2),*/
            "||" => match (c1, c2) {
                (Const::Bool(b1), Const::Bool(b2)) => Const::Bool(*b1 || *b2),
                _ => Const::None,
            },
            "&&" => match (c1, c2) {
                (Const::Bool(b1), Const::Bool(b2)) => Const::Bool(*b1 && *b2),
                _ => Const::None,
            },
            "<" => match (c1, c2) {
                (Const::Imm(i, _s, _b), Const::Imm(i2, _, _)) => Const::Bool(i < i2),
                (Const::Float(f, _), Const::Float(f2, _)) => Const::Bool(f < f2),
                _ => Const::None,
            },
            ">" => match (c1, c2) {
                (Const::Imm(i, _s, _b), Const::Imm(i2, _, _)) => Const::Bool(i > i2),
                (Const::Float(f, _), Const::Float(f2, _)) => Const::Bool(f > f2),
                _ => Const::None,
            },
            "==" => match (c1, c2) {
                (Const::Imm(i, _s, _b), Const::Imm(i2, _, _)) => Const::Bool(i == i2),
                (Const::Float(f, _), Const::Float(f2, _)) => Const::Bool(f == f2),
                (Const::Bool(b1), Const::Bool(b2)) => Const::Bool(b1 == b2),

                _ => Const::None,
            },
            "!=" => match (c1, c2) {
                (Const::Imm(i, _s, _b), Const::Imm(i2, _, _)) => Const::Bool(i != i2),
                (Const::Float(f, _), Const::Float(f2, _)) => Const::Bool(f != f2),
                (Const::Bool(b1), Const::Bool(b2)) => Const::Bool(b1 != b2),
                _ => Const::None,
            },
            ">=" => match (c1, c2) {
                (Const::Imm(i, _s, _b), Const::Imm(i2, _, _)) => Const::Bool(i >= i2),
                (Const::Float(f, _), Const::Float(f2, _)) => Const::Bool(f >= f2),
                _ => Const::None,
            },
            "<=" => match (c1, c2) {
                (Const::Imm(i, _s, _b), Const::Imm(i2, _, _)) => Const::Bool(i <= i2),
                (Const::Float(f, _), Const::Float(f2, _)) => Const::Bool(f <= f2),
                _ => Const::None,
            },
            _ => Const::None,
        };

        Rc::new(RefCell::new(val))
    }
    /// if `to` expression is identifier
    ///  and variable with name of identifier known
    ///  and `from` expression known at compile time perform assign
    fn try_assign(&mut self, to: &Expr, from: &Expr) {
        match &to.kind {
            ExprKind::Ident(name) => {
                if self.known_vars.contains_key(name) {
                    let val = self.eval(from);

                    if !val.borrow().is_none() {
                        self.known_vars.insert(*name, val);
                    } else {
                        self.known_vars.remove(name);
                    }
                }
            }
            ExprKind::Field(expr, field) => {
                if let ExprKind::Ident(name) = &expr.kind {
                    if self.known_vars.contains_key(name) {
                        let val = self.eval(from);
                        if val.borrow().is_none() {
                            return;
                        }
                        let cval = self.known_vars.get(name).unwrap();
                        let cval: &mut Const = &mut cval.borrow_mut();
                        if let Const::Struct(_, fields) = cval {
                            for (name, val_, id) in fields.iter_mut() {
                                if name == field {
                                    *id = from.id;
                                    *val_ = val.clone();
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            _ => (),
        }
    }
    /// Evaluate expression
    fn eval(&mut self, expr: &Expr) -> Rc<RefCell<Const>> {
        match &expr.kind {
            ExprKind::Conv(expr, to) => {
                let val = self.eval(expr);

                if !val.borrow().is_none() {
                    use crate::semantic::{ty_is_any_float, ty_is_any_int};
                    let val: &Const = &val.borrow();
                    if ty_is_any_int(to) {
                        match val {
                            Const::Imm(i, s, b) => rc(Const::Imm(*i, *s, *b)),
                            Const::Float(f, s) => rc(Const::Imm(
                                *f as i64,
                                match s {
                                    FloatSuffix::Float => IntSuffix::Int,
                                    FloatSuffix::Double => IntSuffix::Long,
                                },
                                IntBase::Dec,
                            )),
                            Const::Bool(b) => {
                                rc(Const::Imm(*b as i64, IntSuffix::Int, IntBase::Dec))
                            }
                            Const::Ret(val) => {
                                let val: &Const = &val.borrow();
                                match val {
                                    Const::Imm(i, s, b) => rc(Const::Imm(*i, *s, *b)),
                                    Const::Float(f, s) => rc(Const::Imm(
                                        *f as i64,
                                        match s {
                                            FloatSuffix::Float => IntSuffix::Int,
                                            FloatSuffix::Double => IntSuffix::Long,
                                        },
                                        IntBase::Dec,
                                    )),
                                    Const::Bool(b) => {
                                        rc(Const::Imm(*b as i64, IntSuffix::Int, IntBase::Dec))
                                    }

                                    Const::Void => rc(Const::Void),
                                    _ => rc(Const::None),
                                }
                            }
                            _ => rc(Const::None),
                        }
                    } else if ty_is_any_float(to) {
                        match val {
                            Const::Float(f, s) => rc(Const::Float(*f, *s)),
                            Const::Imm(i, s, _) => rc(Const::Float(
                                *i as f64,
                                match s {
                                    IntSuffix::Int => FloatSuffix::Float,
                                    IntSuffix::Long | IntSuffix::ULong => FloatSuffix::Double,
                                    _ => FloatSuffix::Float,
                                },
                            )),
                            _ => rc(Const::None),
                        }
                    } else {
                        rc(Const::None)
                    }
                } else {
                    rc(Const::None)
                }
            }

            ExprKind::Int(i, b, s) => rc(Const::Imm(*i, *s, *b)),
            ExprKind::Float(f, s) => rc(Const::Float(*f, *s)),
            ExprKind::Bool(b) => rc(Const::Bool(*b)),

            ExprKind::Binary(op, lhs, rhs) => self.eval_binop(op, lhs, rhs),
            ExprKind::Unary(op, expr) => {
                let op: &str = op;
                let val = self.eval(expr);
                let val: &Const = &val.borrow();
                if val.is_none() {
                    return rc(Const::None);
                }
                match op {
                    "+" => match val {
                        Const::Imm(i, s, b) => rc(Const::Imm(*i, *s, *b)),
                        Const::Float(f, s) => rc(Const::Float(*f, *s)),
                        _ => rc(Const::None),
                    },
                    "-" => match val {
                        Const::Imm(i, s, b) => rc(Const::Imm(-i, *s, *b)),
                        Const::Float(f, s) => rc(Const::Float(-f, *s)),
                        _ => rc(Const::None),
                    },
                    "!" => match val {
                        Const::Imm(i, s, b) => rc(Const::Imm(!i, *s, *b)),

                        Const::Bool(b) => rc(Const::Bool(!b)),
                        _ => rc(Const::None),
                    },
                    _ => rc(Const::None),
                }
            }
            ExprKind::Struct(name, fields) => {
                let mut new_fields = vec![];
                for field in fields.iter() {
                    let val = self.eval(&field.expr);
                    if val.borrow().is_none() {
                        return rc(Const::None);
                    }
                    new_fields.push((field.name, val, field.expr.id))
                }

                rc(Const::Struct(name.name(), new_fields))
            }
            ExprKind::Str(s) => rc(Const::Str(s.clone())),
            ExprKind::Field(val, field) => {
                let val = self.eval(val);
                let val: &Const = &val.borrow();
                if val.is_none() {
                    return rc(Const::None);
                }
                if let Const::Struct(_, fields) = val {
                    for (name, cval, _) in fields.iter() {
                        if name == field {
                            return cval.clone();
                        }
                    }
                }

                return rc(Const::None);
            }

            ExprKind::Ident(name) => self.try_get_var(name),
            ExprKind::Assign(to, from) => {
                self.try_assign(to, from);
                return self.eval(from);
            }
            ExprKind::ArrayIdx(expr_, id) => {
                let id = self.eval(id);
                let id: &Const = &id.borrow();
                let array = self.eval(expr_);
                if array.borrow().is_none() || id.is_none() {
                    return rc(Const::None);
                }

                let idx = if let Const::Imm(imm, _, _) = &id {
                    *imm as usize
                } else {
                    unimplemented!()
                };
                let array: &Const = &array.borrow();
                if let Const::Array(array) = array {
                    return array.borrow()[idx].clone();
                } else {
                    return rc(Const::None);
                }
            }

            ExprKind::Call(name, this, args) => {
                if this.is_some() {
                    return rc(Const::None); // we don't support constexpr methods yet
                }

                if self.const_functions.contains_key(&name.name()) {
                    let funcs: Vec<Function> =
                        self.const_functions.get(&name.name()).unwrap().clone();
                    let mut func = None;

                    for fun in funcs.iter() {
                        if args.len() < fun.params.len() || args.len() > fun.params.len() {
                            continue;
                        }
                        let mut params_match = false;
                        if args.len() == 0 && fun.params.len() == 0 {
                            params_match = true;
                        } else {
                            for (i, arg) in args.iter().enumerate() {
                                let ty = self.ctx.types.get(&arg.id).unwrap().clone();
                                params_match = ty == *fun.params[i].1;
                            }
                        }
                        if params_match {
                            func = Some(fun.clone());
                            break;
                        }
                    }

                    if func.is_none() {
                        panic!("Const function not found");
                    } else {
                        let func: Function = func.unwrap();
                        let mut params = vec![];
                        for (name, _) in func.params.iter() {
                            params.push(*name);
                        }
                        return self.eval_constfn(&params, func.body.as_ref().unwrap(), args);
                    }
                } else if self.functions.contains_key(&name.name()) && self.try_eval_normal {
                    let funcs: Vec<Function> = self.functions.get(&name.name()).unwrap().clone();
                    let mut func = None;

                    for fun in funcs.iter() {
                        if args.len() < fun.params.len() || args.len() > fun.params.len() {
                            continue;
                        }
                        let mut params_match = false;
                        if args.len() == 0 && fun.params.len() == 0 {
                            params_match = true;
                        } else {
                            for (i, arg) in args.iter().enumerate() {
                                let ty = self.ctx.types.get(&arg.id).unwrap().clone();
                                params_match = ty == *fun.params[i].1;
                            }
                        }
                        if params_match {
                            func = Some(fun.clone());
                            break;
                        }
                    }

                    if func.is_none() {
                        panic!("function not found");
                    } else {
                        let func: Function = func.unwrap();
                        let mut params = vec![];
                        for (name, _) in func.params.iter() {
                            params.push(*name);
                        }
                        return self.eval_constfn(&params, func.body.as_ref().unwrap(), args);
                    }
                } else if false {
                    let builtin = self.builtins.get(&name.name()).unwrap().clone();
                    let builtin: fn(&[Rc<RefCell<Const>>]) -> Rc<RefCell<Const>> =
                        unsafe { transmute(builtin) };
                    let mut params = vec![];
                    for arg in args.iter() {
                        let val = self.eval(arg);
                        if val.borrow().is_none() {
                            return rc(Const::None);
                        }
                        params.push(val);
                    }

                    let val = builtin(&params);
                    if val.borrow().is_void() {
                        return rc(Const::Imm(0, IntSuffix::Int, IntBase::Dec));
                    } else {
                        return val;
                    }
                } else {
                    // try to optimize arguments

                    for arg in args.iter() {
                        let val = self.eval(arg);
                        if val.borrow().is_none() {
                            return rc(Const::None);
                        } else {
                            if let Elem::Func(f) = &mut self.ctx.file.elems[self.id] {
                                f.replace_expr_to(
                                    arg.id,
                                    Expr {
                                        id: arg.id,
                                        pos: expr.pos,
                                        kind: val.borrow().to_kind(),
                                    },
                                );
                            }
                        }
                    }
                }

                rc(Const::None)
            }
            ExprKind::SizeOf(ty) => {
                if let Some(size) = ty_size(ty) {
                    return rc(Const::Imm(size as i64, IntSuffix::Int, IntBase::Dec));
                } else {
                    return rc(Const::None);
                }
            }

            _ => rc(Const::None),
        }
    }
    /// Evaluate constant function
    fn eval_constfn(
        &mut self,
        params: &[Name],
        body: &Stmt,
        args: &Vec<Box<Expr>>,
    ) -> Rc<RefCell<Const>> {
        let old_vars = self.known_vars.clone();
        //self.known_vars.clear();
        self.return_ = None;
        let mut new_vars = HashMap::new();
        for (i, param) in params.iter().enumerate() {
            let val = self.eval(&args[i]);

            if val.borrow().is_none() {
                return rc(Const::None); // Argument value not known at compile time, return none
            }

            if let Elem::Func(f) = &mut self.ctx.file.elems[self.id] {
                f.replace_expr_to(
                    args[i].id,
                    Expr {
                        id: args[i].id,
                        pos: args[i].pos,
                        kind: val.borrow().to_kind(),
                    },
                );
            }
            new_vars.insert(*param, val);
        }

        self.known_vars = new_vars;
        self.running = true;
        let val = self.eval_stmt(body);
        self.running = false;
        self.known_vars = old_vars;
        if val.is_some() {
            let val: &Const = &val.as_ref().unwrap().borrow();
            if let Const::Ret(val) = val {
                return val.clone();
            } else {
                return rc(val.clone());
            }
        } else {
            return rc(Const::None);
        }
    }
    /// Evaluate constant
    fn eval_stmt(&mut self, stmt: &Stmt) -> Option<Rc<RefCell<Const>>> {
        match &stmt.kind {
            StmtKind::Block(stmts) => {
                let mut last = None;
                for stmt in stmts.iter() {
                    let val = self.eval_stmt(stmt);
                    last = val;
                    if last.is_some() {
                        let last: &Const = &last.as_ref().unwrap().borrow();
                        if let Const::Ret(_) = last {
                            break;
                        }
                    }
                }
                return last;
            }
            StmtKind::Expr(expr) => {
                return Some(self.eval(expr));
            }
            StmtKind::Return(expr) => {
                if expr.is_some() {
                    let val = self.eval(expr.as_ref().unwrap());
                    if val.borrow().is_none() {
                        return None;
                    } else {
                        return Some(rc(Const::Ret(val)));
                    }
                } else {
                    return Some(rc(Const::Ret(Rc::new(RefCell::new(Const::Void)))));
                }
            }
            StmtKind::Var(name, _, ty, expr) => {
                if expr.is_none() {
                    return None;
                } else {
                    let val = self.eval(expr.as_ref().unwrap());
                    if val.borrow().is_none() {
                        return None;
                    }

                    self.known_vars.insert(*name, val);
                }

                return Some(rc(Const::Void));
            }
            StmtKind::If(cond, then_body, else_body) => {
                let val = self.eval(cond);
                let val: &Const = &val.borrow();
                if val.is_none() {
                    return None;
                }

                if let Const::Bool(true) = val {
                    return self.eval_stmt(then_body);
                } else if let Const::Bool(false) = val {
                    if else_body.is_some() {
                        let else_body = else_body.as_ref().unwrap();
                        return self.eval_stmt(else_body);
                    } else {
                        return Some(Rc::new(RefCell::new(Const::Void)));
                    }
                }
                return Some(Rc::new(RefCell::new(Const::Void)));
            }

            StmtKind::While(cond, body) => {
                let cond = self.eval(cond);
                let cond: &Const = &cond.borrow();
                while let Const::Bool(true) = cond {
                    let val = &self.eval_stmt(body);

                    if val.is_none() {
                        return None;
                    } else if val.as_ref().unwrap().borrow().is_none() {
                        return None;
                    }
                }
                return Some(Rc::new(RefCell::new(Const::Void)));
            }

            _ => panic!("Unsupported statement in constant function"),
        }
    }

    /// Evaluates statement in non-const context
    fn eval_normal_stmt(&mut self, s: &Stmt, fid: usize) {
        match &s.kind {
            StmtKind::Block(block) => {
                for stmt in block.iter() {
                    self.eval_normal_stmt(stmt, fid);
                }
            }
            StmtKind::Expr(expr) => {
                let val = self.eval(expr);

                if !val.borrow().is_none() {
                    if let Elem::Func(func) = &mut self.ctx.file.elems[fid] {
                        func.replace_expr_to(
                            expr.id,
                            Expr {
                                id: expr.id,
                                pos: expr.pos,
                                kind: val.borrow().to_kind(),
                            },
                        );
                    }
                }
            }
            StmtKind::If(cond, then, otherwise) => {
                let val = self.eval(cond);
                if !val.borrow().is_none() {
                    if let Elem::Func(func) = &mut self.ctx.file.elems[fid] {
                        func.replace_expr_to(
                            cond.id,
                            Expr {
                                id: cond.id,
                                pos: cond.pos,
                                kind: val.borrow().to_kind(),
                            },
                        );
                    }
                }
                self.eval_normal_stmt(then, fid);
                if otherwise.is_some() {
                    self.eval_normal_stmt(otherwise.as_ref().unwrap(), fid);
                }
            }
            StmtKind::While(cond, body) => {
                let val = self.eval(cond);
                if !val.borrow().is_none() {
                    if let Elem::Func(func) = &mut self.ctx.file.elems[fid] {
                        func.replace_expr_to(
                            cond.id,
                            Expr {
                                id: cond.id,
                                pos: cond.pos,
                                kind: val.borrow().to_kind(),
                            },
                        );
                    }
                }
                self.eval_normal_stmt(body, fid);
            }
            StmtKind::Return(expr) => {
                if expr.is_some() {
                    let expr = expr.as_ref().unwrap();
                    let val = self.eval(expr);
                    if !val.borrow().is_none() {
                        if let Elem::Func(func) = &mut self.ctx.file.elems[fid] {
                            func.replace_expr_to(
                                expr.id,
                                Expr {
                                    id: expr.id,
                                    pos: expr.pos,
                                    kind: val.borrow().to_kind(),
                                },
                            );
                        }
                    }
                }
            }
            StmtKind::Var(var, _, _, val) => {
                if val.is_some() {
                    let expr = val.as_ref().unwrap();
                    let val = self.eval(expr);

                    if !val.borrow().is_none() {
                        if let Elem::Func(func) = &mut self.ctx.file.elems[fid] {
                            if !func.ret.is_void() {
                                func.replace_expr_to(
                                    expr.id,
                                    Expr {
                                        id: expr.id,
                                        pos: expr.pos,
                                        kind: val.borrow().to_kind(),
                                    },
                                );
                            }
                        }
                        self.known_vars.insert(*var, val);
                    }
                }
            }
            StmtKind::Loop(stmt) => self.eval_normal_stmt(stmt, fid),
            StmtKind::Continue => (),
            StmtKind::Break => (),
            _ => (),
        }
    }

    fn opt_func(&mut self, func: &Function, id: usize) {
        self.eval_normal_stmt(func.body.as_ref().unwrap(), id);
    }

    pub fn run(&mut self) {
        // Declare functions before uses
        for elem in self.ctx.file.elems.clone().iter() {
            if let Elem::Func(func) = elem {
                if func.constant {
                    if self.const_functions.contains_key(&func.name) {
                        let funcs = self.const_functions.get_mut(&func.name).unwrap();
                        funcs.push(func.clone());
                    } else {
                        self.const_functions.insert(func.name, vec![func.clone()]);
                    }
                } else if !func.internal && !func.external {
                    if self.functions.contains_key(&func.name) {
                        let funcs = self.functions.get_mut(&func.name).unwrap();
                        funcs.push(func.clone());
                    } else {
                        self.functions.insert(func.name, vec![func.clone()]);
                    }
                }
            }
        }
        for (i, elem) in self.ctx.file.elems.clone().iter().enumerate() {
            match elem {
                Elem::Func(func) => {
                    if func.external || func.internal {
                        continue;
                    }
                    /*if func.constant
                    {
                        if self.const_functions.contains_key(&func.name)
                        {
                            let funcs = self.const_functions.get_mut(&func.name).unwrap();
                            funcs.push(func.clone());
                        }
                        else
                        {
                            self.const_functions.insert(func.name, vec![func.clone()]);
                        }
                    }*/
                    self.id = i;

                    self.opt_func(func, i);
                }
                Elem::ConstExpr { name, expr, .. } => {
                    self.constexprs.insert(*name, *expr.clone());
                }
                _ => (),
            }
        }
    }
}
