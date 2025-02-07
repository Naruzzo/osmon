use crate::Context as CContext;
use gccjit_rs::{
    block::{BinaryOp, Block, ComparisonOp, UnaryOp},
    ctx::{Context, GlobalKind, OutputKind},
    field::Field,
    function::{Function as CFunction, FunctionType},
    lvalue::LValue,
    rvalue::{RValue, ToRValue},
    ty::Type as CType,
};

use crate::{
    str,
    syntax::ast::{
        Elem, Expr, ExprKind, Function, NodeId, Stmt, StmtKind, StructArg, StructField, Type,
    },
};

use crate::syntax::interner::Name;
use std::{
    collections::{HashMap, VecDeque},
    ffi::CString,
};

/// Create gccjit location from AST location
fn gccloc_from_loc(
    ctx: &Context,
    loc: &crate::syntax::position::Position,
) -> gccjit_rs::location::Location {
    ctx.new_location(str(loc.file).to_string(), loc.line as _, loc.column as _)
}

/// This type stores information about function such as gccjit and ast
/// representation
#[derive(Clone)]
pub struct FunctionUnit {
    /// AST Function
    pub f: Function,
    /// GCCJIT Function
    pub c: CFunction,
    pub this_ast: Option<Type>,
    pub this_ir: Option<CType>,
    pub irname: String,
}

/// Variable info that stores lvalue,type and gccjit type
#[derive(Clone)]
pub struct VarInfo {
    pub lval: LValue,
    pub ty: Type,
    pub cty: CType,
}
/// GccStruct stores gccjit struct type and information about fields
#[derive(Clone)]
pub struct GccStruct {
    pub ty: CType,
    pub fields: HashMap<Name, Field>,
    pub types: Vec<Type>,
}

use super::eval::Const;
use std::{cell::RefCell, rc::Rc};
/// Main unit used in codegeneration.
///
/// This unit performs translating AST into GIMPLE tree,after translation GCC
/// performs translation to RTL and then optimizes it and outputs machine code
pub struct Codegen<'a> {
    pub ctx: Context,
    context: &'a mut CContext,
    continue_blocks: VecDeque<Block>,
    break_blocks: VecDeque<Block>,
    known_vars: HashMap<Name, Rc<RefCell<Const>>>,
    cur_func: Option<CFunction>,
    cur_block: Option<Block>,
    variables: HashMap<Name, VarInfo>,
    globals: HashMap<Name, (VarInfo, Option<Box<Expr>>)>,
    functions: HashMap<Name, Vec<FunctionUnit>>,
    external_functions: HashMap<Name, FunctionUnit>,
    const_functions: HashMap<Name, Vec<Function>>,
    structures: HashMap<Name, GccStruct>,
    constants: HashMap<Name, Expr>,
    block_id: usize,
    fun_id: usize,
    aliases: HashMap<Name, Type>,
    tmp_id: usize,
    terminated: Vec<bool>,
    cur_return: Option<Type>,
}

impl<'a> Codegen<'a> {
    /// Get type size for sizeof expression
    pub fn ty_size(&self, ty: &Type) -> usize {
        match ty {
            Type::Vector(v) => return self.ty_size(&v.subtype) * v.size,
            Type::Void(_) => 0,
            Type::Basic(basic) => {
                let name: &str = &str(basic.name);
                match name {
                    "u8" => 1,
                    "i8" => 1,
                    "char" => 1,
                    "i16" => 2,
                    "u16" => 2,
                    "i32" => 4,
                    "u32" => 4,
                    "i64" => 8,
                    "u64" => 8,
                    "f32" => 4,
                    "f64" => 8,
                    "bool" => 1,
                    "usize" => 8,
                    s => {
                        let interned = crate::syntax::interner::intern(s);
                        if self.structures.contains_key(&interned) {
                            let structure = self.structures.get(&interned).unwrap().clone();
                            let mut size = 0;
                            for field in structure.types.iter() {
                                size += self.ty_size(field)
                            }

                            size
                        } else if let Some(ty) = self
                            .aliases
                            .get(&crate::syntax::interner::intern(s))
                            .clone()
                        {
                            self.ty_size(ty)
                        } else {
                            panic!("Type {} not found", s);
                        }
                    }
                }
            }
            Type::Ptr(_ptr) => 8,
            Type::Func(_tyfunc) => 8,
            Type::Struct(structure) => {
                let structure = self.structures.get(&structure.name).unwrap();
                let mut size = 0;
                for field in structure.types.iter() {
                    size += self.ty_size(field)
                }

                size
            }
            Type::Array(array) => {
                if array.len.is_some() {
                    self.ty_size(&array.subtype) * array.len.unwrap() as usize
                } else {
                    8
                }
            }
        }
    }
    /// Convert AST type into GCC type
    pub fn ty_to_ctype(&mut self, ty: &Type) -> CType {
        let ctx = self.ctx;
        match ty {
            Type::Vector(v) => {
                let subname: &str = &str(v.subtype.to_basic().unwrap().name).to_string();
                match subname {
                    "u8" => self.ctx.new_vector_type::<u8>(v.size),
                    "i8" => self.ctx.new_vector_type::<i8>(v.size),
                    "char" => self.ctx.new_vector_type::<char>(v.size),
                    "i16" => self.ctx.new_vector_type::<i16>(v.size),
                    "u16" => self.ctx.new_vector_type::<u16>(v.size),
                    "u32" => self.ctx.new_vector_type::<u32>(v.size),
                    "u64" => self.ctx.new_vector_type::<u64>(v.size),
                    "i64" => self.ctx.new_vector_type::<i64>(v.size),
                    "i32" => self.ctx.new_vector_type::<i32>(v.size),
                    "usize" => self.ctx.new_vector_type::<usize>(v.size),
                    _ => unimplemented!(),
                }
            }
            Type::Void(_) => ctx.new_type::<()>(),
            Type::Basic(basic) => {
                let name: &str = &str(basic.name);
                match name {
                    "u8" => ctx.new_type::<u8>(),
                    "i8" => ctx.new_type::<i8>(),
                    "char" => ctx.new_type::<char>(),
                    "i16" => ctx.new_type::<i16>(),
                    "u16" => ctx.new_type::<u16>(),
                    "i32" => ctx.new_type::<i32>(),
                    "u32" => ctx.new_type::<u32>(),
                    "i64" => ctx.new_type::<i64>(),
                    "u64" => ctx.new_type::<u64>(),
                    "f32" => ctx.new_type::<f32>(),
                    "f64" => ctx.new_type::<f64>(),
                    "bool" => ctx.new_type::<bool>(),
                    "usize" => ctx.new_type::<usize>(),
                    s => {
                        let interned = crate::syntax::interner::intern(s);
                        if self.structures.contains_key(&interned) {
                            self.structures.get(&interned).unwrap().ty
                        } else if let Some(ty) =
                            self.aliases.get(&crate::syntax::interner::intern(s))
                        {
                            let ty = ty.clone();
                            return self.ty_to_ctype(&ty);
                        } else {
                            panic!("Unknown type: {} at {}", s, ty.pos())
                        }
                    }
                }
            }
            Type::Ptr(ptr) => self.ty_to_ctype(&ptr.subtype).make_pointer(),
            Type::Func(tyfunc) => {
                let params = tyfunc
                    .params
                    .iter()
                    .map(|elem| self.ty_to_ctype(elem))
                    .collect::<Vec<_>>();
                ctx.new_function_pointer_type(None, self.ty_to_ctype(&tyfunc.ret), &params, false)
            }
            Type::Struct(struct_) => {
                if self.structures.contains_key(&struct_.name) {
                    self.structures
                        .get(&struct_.name)
                        .expect(&format!("Struct {} not found", str(struct_.name)))
                        .ty
                } else {
                    let mut fields = vec![];
                    let mut cfields = HashMap::new();
                    let mut types = vec![];
                    for field in struct_.fields.iter() {
                        let field: &StructField = field;
                        let cty = self.ty_to_ctype(&field.data_type).clone();
                        types.push(field.data_type.clone());
                        let name: &str = &str(field.name).to_string();
                        let cfield = self.ctx.new_field(
                            Some(gccloc_from_loc(&self.ctx, &field.pos)),
                            cty,
                            name,
                        );
                        cfields.insert(field.name, cfield);
                        fields.push(cfield);
                    }
                    let ty = if struct_.union {
                        self.ctx
                            .new_union_type(None, &str(struct_.name).to_string(), &fields)
                    } else {
                        self.ctx
                            .new_struct_type(None, &str(struct_.name).to_string(), &fields)
                            .as_type()
                    };
                    self.structures.insert(
                        struct_.name,
                        GccStruct {
                            ty,
                            fields: cfields,
                            types,
                        },
                    );
                    ty
                }
            }
            Type::Array(array) => {
                if array.len.is_some() {
                    let len = *array.len.as_ref().unwrap();

                    ctx.new_array_type(None, self.ty_to_ctype(&array.subtype), len as i32)
                } else {
                    self.ty_to_ctype(&array.subtype).make_pointer()
                }
            }
        }
    }
    /// assign some value to lvalue
    fn assign(&mut self, pos: crate::syntax::position::Position, to: &Expr, from: &Expr) -> RValue {
        let lval = self.expr_to_lvalue(to).unwrap();
        let rval = self.gen_expr(from);
        let type_ = self.get_id_type(to.id);
        let _ = self.get_id_type(from.id);
        let do_cast = match type_ {
            Type::Basic(basic) => {
                if self.structures.contains_key(&basic.name) {
                    false
                } else {
                    true
                }
            }
            Type::Struct(_) => false,
            Type::Ptr(_) => true,
            Type::Array(_) => false,
            _ => true,
        };

        let val = if do_cast {
            self.ctx.new_cast(None, rval, lval.to_rvalue().get_type())
        } else {
            rval
        };
        self.cur_block
            .unwrap()
            .add_assignment(Some(gccloc_from_loc(&self.ctx, &pos)), lval, val);

        rval
    }
    /// Search for func with params and this value
    fn search_for_func(
        &mut self,
        params: &[Type],
        this: Option<&Type>,
        functions: &[FunctionUnit],
    ) -> Option<(CFunction, Vec<CType>, Vec<Type>)> {
        let val = None;

        for function in functions.iter() {
            let function: &FunctionUnit = function;

            let mut params_okay = false;
            let mut not_found = false;
            if function.f.params.len() > params.len() {
                continue;
            }

            if function.f.params.len() == 0
                && params.len() == 0
                && function.this_ast.is_none()
                && this.is_none()
            {
                return Some((function.c, vec![], vec![]));
            }
            if let Some(ty) = &function.this_ast {
                if this.is_none() {
                    continue;
                } else {
                    let this = this.unwrap();
                    let this = if this.is_ptr() {
                        this.clone()
                    } else {
                        this.make_ptr()
                    };

                    if &*ty != &this {
                        continue;
                    } else {
                        let mut sig_params = vec![];
                        for (_, p) in function.f.params.iter() {
                            sig_params.push(*p.clone());
                        }

                        if sig_params == params {
                            return Some((
                                function.c,
                                function
                                    .f
                                    .params
                                    .iter()
                                    .map(|(_, typ)| self.ty_to_ctype(typ))
                                    .collect(),
                                function
                                    .f
                                    .params
                                    .iter()
                                    .map(|(_, typ)| *typ.clone())
                                    .collect(),
                            ));
                        }
                    }
                }
            }

            for (index, param) in params.iter().enumerate() {
                if index < function.f.params.len() {
                    params_okay = param == &*function.f.params[index].1;
                } else {
                    if function.f.variadic && params_okay {
                        not_found = false;
                        break;
                    } else {
                        params_okay = false;
                        not_found = true;
                        break;
                    }
                }

                if !params_okay {
                    not_found = true;
                    break;
                }
            }

            if not_found {
                continue;
            }

            if params_okay {
                return Some((
                    function.c,
                    function
                        .f
                        .params
                        .iter()
                        .map(|(_, typ)| self.ty_to_ctype(typ))
                        .collect(),
                    function
                        .f
                        .params
                        .iter()
                        .map(|(_, typ)| *typ.clone())
                        .collect(),
                ));
            } else {
                continue;
            }
        }
        val
    }

    fn search_for_func_const(
        &mut self,
        params: &[Type],
        this: Option<&Type>,
        functions: &[Function],
    ) -> Option<(Function, Vec<Type>)> {
        let val = None;

        for function in functions.iter() {
            let function: &Function = function;

            let mut params_okay = false;
            let mut not_found = false;
            if function.params.len() > params.len() {
                continue;
            }

            if function.params.len() == 0 && params.len() == 0 && this.is_none() {
                return Some((function.clone(), vec![]));
            }

            for (index, param) in params.iter().enumerate() {
                if index < function.params.len() {
                    params_okay = param == &*function.params[index].1;
                } else {
                    if function.variadic && params_okay {
                        not_found = false;
                        break;
                    } else {
                        params_okay = false;
                        not_found = true;
                        break;
                    }
                }

                if !params_okay {
                    not_found = true;
                    break;
                }
            }

            if not_found {
                continue;
            }

            if params_okay {
                return Some((
                    function.clone(),
                    function
                        .params
                        .iter()
                        .map(|(_, typ)| *typ.clone())
                        .collect(),
                ));
            } else {
                continue;
            }
        }
        val
    }

    pub fn new(context: &'a mut CContext, name: &str) -> Codegen<'a> {
        let ctx = Context::default();

        ctx.set_name(name);
        ctx.set_dump_gimple(context.gimple);
        use gccjit_rs::sys::*;
        unsafe {
            let ptr = gccjit_rs::ctx::context_get_ptr(&ctx);
            gcc_jit_context_set_bool_allow_unreachable_blocks(ptr, true as _);
        }
        Codegen {
            ctx,
            known_vars: HashMap::new(),
            context,
            continue_blocks: VecDeque::new(),
            break_blocks: VecDeque::new(),
            cur_block: None,
            cur_func: None,
            variables: HashMap::new(),
            globals: HashMap::new(),
            functions: HashMap::new(),
            external_functions: HashMap::new(),
            structures: HashMap::new(),
            constants: HashMap::new(),
            block_id: 0,
            fun_id: 0,
            aliases: HashMap::new(),
            tmp_id: 0,
            terminated: vec![],
            const_functions: HashMap::new(),
            cur_return: None,
        }
    }
    /// Find struct type
    pub fn find_struct(&self, ty: &Type) -> Option<GccStruct> {
        match ty {
            Type::Basic(basic) => {
                if let Some(s) = self.structures.get(&basic.name) {
                    return Some(s.clone());
                } else if let Some(ty) = self.aliases.get(&basic.name) {
                    return self.find_struct(ty);
                } else {
                    return None;
                }
            }
            Type::Struct(basic) => {
                if let Some(s) = self.structures.get(&basic.name) {
                    return Some(s.clone());
                } else if let Some(ty) = self.aliases.get(&basic.name) {
                    return self.find_struct(ty);
                } else {
                    return None;
                }
            }
            _ => None,
        }
    }
    /// Convert expression to LValue
    pub fn expr_to_lvalue(&mut self, expr: &Expr) -> Option<LValue> {
        match &expr.kind {
            ExprKind::Ident(name) => {
                if self.variables.contains_key(name) {
                    let value = self.variables.get(name).unwrap().clone();

                    Some(value.lval)
                } else if self.globals.contains_key(name) {
                    let value = self.globals.get(name).unwrap().0.lval;

                    Some(value)
                } else {
                    None
                }
            }
            ExprKind::Binary(op, lhs, rhs) => {
                let _op: &str = op;
                let t1 = self.get_id_type(lhs.id);
                let t2 = self.get_id_type(rhs.id);

                if t1.is_ptr() && crate::semantic::ty_is_any_int(&t2) {
                    let array = self.gen_expr(lhs);
                    let idx = self.gen_expr(rhs);

                    Some(self.ctx.new_array_access(
                        Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                        array,
                        idx,
                    ))
                } else {
                    None
                }
            }
            ExprKind::ArrayIdx(array, index) => {
                let array = self.gen_expr(array);
                let index = self.gen_expr(index);

                Some(self.ctx.new_array_access(
                    Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                    array,
                    index,
                ))
            }
            ExprKind::Field(object, name) => {
                let ty: Type = self.get_id_type(object.id).clone();

                if ty.is_ptr() {
                    let ptr = ty.to_ptr().unwrap();
                    if ptr.subtype.is_struct() {
                        let struct_ = self
                            .structures
                            .get(&ptr.subtype.to_struct().unwrap().name)
                            .unwrap()
                            .clone();

                        let cfield = struct_.fields.get(name).expect("Field not found");
                        let lval = self.gen_expr(object);
                        let _ast_ty = self.get_id_type(expr.id);
                        //let cty = self.ty_to_ctype(&ast_ty);

                        Some(lval.dereference_field(
                            Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                            *cfield,
                        ))
                    } else if let Type::Basic(basic) = &*ptr.subtype.clone() {
                        let struct_ = self.structures.get(&basic.name).unwrap().clone();

                        let cfield = struct_.fields.get(name).expect("Field not found");
                        let lval = self.gen_expr(object);

                        //Some(lval.access_field(, *cfield))
                        return Some(lval.dereference_field(
                            Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                            *cfield,
                        ));
                    } else {
                        panic!()
                    }
                } else if ty.is_struct() {
                    let struct_: GccStruct = self
                        .structures
                        .get(&ty.to_struct().unwrap().name)
                        .unwrap()
                        .clone();

                    let cfield = struct_.fields.get(name).expect("Field not found");
                    let lval = self.expr_to_lvalue(object).expect("LValue expected");

                    Some(lval.access_field(Some(gccloc_from_loc(&self.ctx, &expr.pos)), *cfield))
                } else {
                    let s = self.find_struct(&ty).expect("Struct not found");

                    let cfield = s.fields.get(name).expect("Field not found");
                    let lval = self.expr_to_lvalue(object).expect("LValue expected");

                    Some(lval.access_field(Some(gccloc_from_loc(&self.ctx, &expr.pos)), *cfield))
                }
            }
            ExprKind::Deref(expr_) => {
                let val = self.gen_expr(expr_);

                Some(val.dereference(Some(gccloc_from_loc(&self.ctx, &expr.pos))))
            }

            _ => None, // unimplemented or impossible to get lval
        }
    }
    /// Get type by expression Id
    fn get_id_type(&self, id: NodeId) -> Type {
        self.context.types.get(&id).unwrap().clone()
    }
    /// Create new name for block
    fn block_name_new(&mut self) -> String {
        let name = format!("L{}", self.block_id);
        self.block_id += 1;
        name
    }
    /// Generate GIMPLE from statement
    pub fn gen_stmt(&mut self, stmt: &Stmt, init: bool) {
        match &stmt.kind {
            StmtKind::CompTime(s) => self.gen_stmt(s, init),

            StmtKind::Expr(expr) => {
                let rval = self.gen_expr(expr);
                self.cur_block
                    .unwrap()
                    .add_eval(Some(gccloc_from_loc(&self.ctx, &expr.pos)), rval);
            }
            StmtKind::Block(stmts) => {
                if !init {
                    let old_block = self.cur_block;
                    let block_name = self.block_name_new();
                    let block = self.cur_func.unwrap().new_block(&block_name);
                    self.cur_block = Some(block);

                    for stmt in stmts.iter() {
                        self.gen_stmt(stmt, false);
                    }

                    self.cur_block = old_block;
                } else {
                    for stmt in stmts.iter() {
                        self.gen_stmt(stmt, false);
                    }
                }
            }
            StmtKind::Break => {
                let break_bb = if let Some(block) = self.break_blocks.back() {
                    *block
                } else {
                    panic!("");
                };
                //let dead_block = self.cur_func.unwrap().new_block(self.block_name_new());

                self.cur_block
                    .unwrap()
                    .end_with_jump(Some(gccloc_from_loc(&self.ctx, &stmt.pos)), break_bb);
                self.cur_block = Some(break_bb);
                *self.terminated.last_mut().unwrap() = true;
            }
            StmtKind::Continue => {
                let continue_bb = if let Some(block) = self.continue_blocks.back() {
                    *block
                } else {
                    panic!("")
                };
                let dead_block = self.cur_func.unwrap().new_block(self.block_name_new());

                self.cur_block
                    .unwrap()
                    .end_with_jump(Some(gccloc_from_loc(&self.ctx, &stmt.pos)), continue_bb);
                self.cur_block = Some(dead_block);
            }
            StmtKind::Return(expr) => {
                if expr.is_some() {
                    let expr = expr.as_ref().unwrap();
                    let val = self.gen_expr(expr);
                    //let ty = self.cur_return.as_ref().unwrap().clone();

                    self.cur_block
                        .unwrap()
                        .end_with_return(Some(gccloc_from_loc(&self.ctx, &stmt.pos)), val);
                } else {
                    self.cur_block.unwrap().end_with_void_return(None);
                }

                //let dead_block = self.cur_func.unwrap().new_block(self.block_name_new());
                //self.cur_block = Some(dead_block);

                if self.terminated.last().is_some() {
                    *self.terminated.last_mut().unwrap() = true;
                }
            }
            StmtKind::Var(name, _, _, init) => {
                let ty = self.get_id_type(stmt.id).clone();

                let cty = self.ty_to_ctype(&ty);
                let local = self.cur_func.unwrap().new_local(
                    Some(gccloc_from_loc(&self.ctx, &stmt.pos)),
                    cty,
                    &str(*name).to_string(),
                );
                if init.is_some() {
                    let expr = init.as_ref().unwrap();
                    let rval = self.gen_expr(expr);
                    let ast_ty = self.get_id_type(expr.id);
                    let cty = self.ty_to_ctype(&ast_ty);
                    let rval = if !ty.is_struct() && !ty.is_array() {
                        self.ctx.new_cast(None, rval, cty)
                    } else {
                        rval
                    };
                    self.cur_block.unwrap().add_assignment(
                        Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                        local,
                        rval,
                    );
                }

                self.variables.insert(
                    *name,
                    VarInfo {
                        cty,
                        lval: local,
                        ty: ty.clone(),
                    },
                );
            }
            StmtKind::If(cond, then, otherwise) => {
                let func: CFunction = self.cur_func.unwrap();

                let then_name = self.block_name_new();
                let else_name = self.block_name_new();

                let bb_then = func.new_block(&format!("if_true:{}", then_name));
                let bb_else = func.new_block(&format!("if_false:{}", else_name));
                let bb_merge: Block = if otherwise.is_some() {
                    let merge_name = self.block_name_new();

                    func.new_block(&format!("after:{}", merge_name))
                } else {
                    bb_else
                };

                let expr = self.gen_expr(cond);

                self.cur_block.unwrap().end_with_conditional(
                    Some(gccloc_from_loc(&self.ctx, &cond.pos)),
                    expr,
                    bb_then,
                    bb_else,
                );

                self.terminated.push(false);
                self.cur_block = Some(bb_then);
                self.gen_stmt(then, true);
                if !*self.terminated.last().unwrap() {
                    self.cur_block
                        .unwrap()
                        .end_with_jump(Some(gccloc_from_loc(&self.ctx, &then.pos)), bb_merge);
                }
                self.cur_block = Some(bb_merge);

                self.terminated.pop();
                self.terminated.push(false);
                if let Some(else_branch) = otherwise {
                    self.cur_block = Some(bb_else);
                    self.gen_stmt(else_branch, true);

                    if !*self.terminated.last().unwrap() {
                        self.cur_block.unwrap().end_with_jump(
                            Some(gccloc_from_loc(&self.ctx, &else_branch.pos)),
                            bb_merge,
                        );
                    }
                }
                self.terminated.pop();
                self.cur_block = Some(bb_merge);
            }
            StmtKind::CFor(var, cond, then, body) => {
                let func: CFunction = self.cur_func.unwrap();

                let loop_cond: Block =
                    func.new_block(&format!("for_cond:{}", self.block_name_new()));
                let loop_body: Block =
                    func.new_block(&format!("for_loop_body:{}", self.block_name_new()));
                let after_loop: Block =
                    func.new_block(&format!("after_for:{}", self.block_name_new()));
                //let for_body: Block =
                // func.new_block(&format!("for_body:{}",self.block_name_new()));
                self.break_blocks.push_back(after_loop);
                self.continue_blocks.push_back(loop_cond);
                //self.cur_block.unwrap().end_with_jump(None,for_body);
                //self.cur_block = Some(for_body);

                self.gen_stmt(var, true);
                self.cur_block.unwrap().end_with_jump(None, loop_cond);
                self.cur_block = Some(loop_cond);
                let val = self.gen_expr(cond);
                self.cur_block
                    .unwrap()
                    .end_with_conditional(None, val, loop_body, after_loop);
                self.cur_block = Some(loop_body);
                self.terminated.push(false);
                self.gen_stmt(body, true);
                self.gen_expr(then);
                self.cur_block.unwrap().end_with_jump(None, loop_cond);

                self.continue_blocks.pop_back();
                self.break_blocks.pop_back();
                self.cur_block = Some(after_loop);
                self.terminated.pop();
            }
            StmtKind::While(cond, block_) => {
                let func: CFunction = self.cur_func.unwrap();

                let loop_cond: Block = func.new_block(self.block_name_new());
                let loop_body: Block = func.new_block(self.block_name_new());
                let after_loop: Block = func.new_block(self.block_name_new());
                self.break_blocks.push_back(after_loop);
                self.continue_blocks.push_back(loop_cond);

                self.cur_block.unwrap().end_with_jump(None, loop_cond);
                self.cur_block = Some(loop_cond);
                self.terminated.push(false);
                let val = self.gen_expr(cond);
                //if !*self.terminated.last().unwrap() {
                self.cur_block
                    .unwrap()
                    .end_with_conditional(None, val, loop_body, after_loop);
                //}

                self.cur_block = Some(loop_body);
                self.gen_stmt(block_, true);
                self.cur_block.unwrap().end_with_jump(None, loop_cond);

                self.continue_blocks.pop_back();
                self.break_blocks.pop_back();
                self.cur_block = Some(after_loop);
            }
            StmtKind::Loop(body) => {
                let bb = self.cur_func.unwrap().new_block(self.block_name_new());
                let after = self.cur_func.unwrap().new_block(self.block_name_new());
                self.break_blocks.push_back(after);
                self.continue_blocks.push_back(bb);

                self.cur_block.unwrap().end_with_jump(None, bb);
                self.cur_block = Some(bb);

                self.gen_stmt(body, true);

                self.cur_block.unwrap().end_with_jump(None, bb);

                self.cur_block = Some(after);
            }
        }
    }
    /// Generate GIMPLE expression from AST expression
    pub fn gen_expr(&mut self, expr: &Expr) -> RValue {
        match &expr.kind {
            ExprKind::ArrayIdx(array, index) => {
                let array = self.gen_expr(array);
                let index = self.gen_expr(index);

                self.ctx.new_array_access(None, array, index).to_rvalue()
            }
            ExprKind::Ident(name) => {
                if self.constants.contains_key(name) {
                    let constexpr = self.constants.get(name).unwrap().clone();
                    if let Some(lval) = self.expr_to_lvalue(&constexpr) {
                        return lval.to_rvalue();
                    } else {
                        return self.gen_expr(&constexpr);
                    }
                };
                self.expr_to_lvalue(expr).unwrap().to_rvalue()
            }
            ExprKind::Float(f, suffix) => {
                use crate::syntax::lexer::token::FloatSuffix;
                let float: f64 = *f as _;
                match suffix {
                    FloatSuffix::Float => self
                        .ctx
                        .new_rvalue_from_double(self.ctx.new_type::<f32>(), float),
                    FloatSuffix::Double => self
                        .ctx
                        .new_rvalue_from_double(self.ctx.new_type::<f64>(), float),
                }
            }
            ExprKind::Int(i, _, suffix) => {
                macro_rules! new_basic_ty {
                    ($name: expr) => {
                        Type::create_basic(
                            expr.id,
                            expr.pos,
                            crate::syntax::interner::intern($name),
                        );
                    };
                }

                use crate::syntax::lexer::token::IntSuffix;
                let int: i64 = *i as _;
                let (val, ty) = match suffix {
                    IntSuffix::Int => (
                        self.ctx
                            .new_rvalue_from_int(self.ctx.new_type::<i32>(), int as i32),
                        new_basic_ty!("i32"),
                    ),
                    IntSuffix::UInt => (
                        self.ctx
                            .new_rvalue_from_int(self.ctx.new_type::<u32>(), int as i32),
                        new_basic_ty!("u32"),
                    ),
                    IntSuffix::Byte => (
                        self.ctx
                            .new_rvalue_from_int(self.ctx.new_type::<i8>(), int as i32),
                        new_basic_ty!("i8"),
                    ),
                    IntSuffix::UByte => (
                        self.ctx
                            .new_rvalue_from_int(self.ctx.new_type::<u8>(), int as i32),
                        new_basic_ty!("u8"),
                    ),
                    IntSuffix::Long => (
                        self.ctx
                            .new_rvalue_from_long(self.ctx.new_type::<i64>(), int),
                        new_basic_ty!("i64"),
                    ),
                    IntSuffix::ULong => (
                        self.ctx
                            .new_rvalue_from_long(self.ctx.new_type::<u64>(), int),
                        new_basic_ty!("u64"),
                    ),
                };

                if !self.context.types.contains_key(&expr.id) {
                    self.context.types.insert(expr.id, ty);
                }
                val
            }
            ExprKind::Str(s) => self.ctx.new_string_literal(s),
            ExprKind::Deref(expr) => {
                let rvalue = self.gen_expr(expr);
                rvalue.dereference(None).to_rvalue()
            }
            ExprKind::Unary(op, expr_) => {
                let op: &str = op;
                let rval = self.gen_expr(expr_);
                let ty = rval.get_type();

                match op {
                    "-" => self.ctx.new_unary_op(None, UnaryOp::Minus, ty, rval),
                    "!" => {
                        let ast_ty = self.get_id_type(expr.id);
                        if crate::semantic::ty_is_any_int(&ast_ty) {
                            self.ctx
                                .new_unary_op(None, UnaryOp::BitwiseNegate, ty, rval)
                        } else {
                            self.ctx
                                .new_unary_op(None, UnaryOp::LogicalNegate, ty, rval)
                        }
                    }
                    "+" => rval,
                    _ => unreachable!(),
                }
            }
            ExprKind::Field(_expr_, _name) => {
                self.expr_to_lvalue(expr).unwrap().to_rvalue()
                /*let ast_ty = self.get_id_type(expr_.id);
                let rvalue = self.gen_expr(expr_).clone();

                if ast_ty.is_ptr()
                {
                    let ptr = ast_ty.to_ptr().unwrap();
                    if ptr.subtype.is_struct()
                    {
                        let struct_: GccStruct = self
                            .structures
                            .get(&ptr.subtype.to_struct().unwrap().name)
                            .unwrap()
                            .clone();

                        let cfield = struct_.fields.get(name).expect("Field not found");
                        let rval = self.gen_expr(expr_);
                        rval.dereference_field(None, *cfield).to_rvalue()
                    }
                    else if ptr.subtype.is_basic()
                    {
                        let basic = ptr.subtype.to_basic().unwrap();
                        let struct_: GccStruct = self.structures.get(&basic.name).unwrap().clone();

                        let cfield = struct_.fields.get(name).expect("Field not found");
                        let rval = self.gen_expr(expr_);
                        rval.dereference_field(None, *cfield).to_rvalue()
                    }
                    else
                    {
                        panic!();
                    }
                }
                else if ast_ty.is_struct()
                {
                    let struct_: GccStruct = self
                        .structures
                        .get(&ast_ty.to_struct().unwrap().name)
                        .unwrap()
                        .clone();

                    let cfield = struct_.fields.get(name).expect("Field not found");
                    rvalue.access_field(None, *cfield)
                }
                else
                {
                    let basic = ast_ty.to_basic().unwrap();

                    let cstruct: &GccStruct = self.structures.get(&basic.name).expect("not found");

                    let field = cstruct.fields.get(name).unwrap();

                    rvalue.access_field(None, *field)
                }*/
            }
            ExprKind::Assign(lval_, rval_) => {
                self.assign(expr.pos, lval_, rval_)
                /*let lval = self.expr_to_lvalue(lval_).unwrap();
                let val = self.gen_expr(rval_);

                let ast_ty = self.get_id_type(rval_.id);
                let lval_ty = self.get_id_type(lval_.id);

                let val = if !ast_ty.is_struct()
                {
                    let ty = self.ty_to_ctype(&lval_ty);
                    self.ctx.new_cast(None, val, ty)
                }
                else
                {
                    val
                };
                self.cur_block.unwrap().add_assignment(
                    Some(gccloc_from_loc(&self.ctx, &lval_.pos)),
                    lval,
                    val,
                );

                val*/
            }

            ExprKind::Bool(b) => self
                .ctx
                .new_rvalue_from_int(self.ctx.new_type::<bool>(), *b as i32),
            ExprKind::AddressOf(expr_) => {
                let ty = self.get_id_type(expr.id);
                let _cty = self.ty_to_ctype(&ty);
                let val = self.expr_to_lvalue(expr_);
                if val.is_none() {
                    let rval = self.gen_expr(expr_);
                    let tmp = self.ctx.new_global(
                        Some(gccloc_from_loc(&self.ctx, &expr_.pos)),
                        GlobalKind::Internal,
                        rval.get_type(),
                        &format!("_{}_", self.tmp_id.to_string()),
                    );
                    self.cur_block.unwrap().add_assignment(
                        Some(gccloc_from_loc(&self.ctx, &expr_.pos)),
                        tmp,
                        rval,
                    );
                    self.tmp_id += 1;

                    let val = tmp.get_address(Some(gccloc_from_loc(&self.ctx, &expr.pos)));
                    //self.ctx.new_cast(None,val,cty)
                    val
                } else {
                    let val = val
                        .unwrap()
                        .get_address(Some(gccloc_from_loc(&self.ctx, &expr.pos)));
                    //self.ctx.new_cast(None,val,cty)
                    val
                }
            }
            ExprKind::Conv(val, to) => {
                let cty = self.ty_to_ctype(to);
                let rval = self.gen_expr(val);
                self.ctx
                    .new_cast(Some(gccloc_from_loc(&self.ctx, &expr.pos)), rval, cty)
            }

            ExprKind::Call(name, this, args) => {
                let param_types = args
                    .iter()
                    .map(|expr| self.get_id_type(expr.id).clone())
                    .collect::<Vec<_>>();

                let var = if let Some(functions) = self.functions.get(&name.name()) {
                    let functions = functions.clone();
                    let ty = if let Some(this) = this {
                        Some(self.get_id_type(this.id))
                    } else {
                        None
                    };
                    let val = self.search_for_func(&param_types, ty.as_ref(), &functions);

                    if val.is_none() {
                        print!("Function {}(", str(name.name()));
                        for p in param_types.iter() {
                            print!(" {} ", p);
                        }
                        print!(") not found\n");
                        std::process::exit(-1);
                    }
                    let (val, c_types, ast_types) = val.unwrap();
                    let mut params = vec![];

                    for (i, arg) in args.iter().enumerate() {
                        if i < ast_types.len() {
                            let val = self.gen_expr(arg);
                            let ty = &ast_types[i];
                            let implicit_casted = if !ty.is_struct() && !ty.is_array() {
                                let cty = c_types[i];
                                self.ctx.new_cast(None, val, cty)
                            } else {
                                val
                            };
                            params.push(implicit_casted);
                        } else {
                            let val = self.gen_expr(arg);
                            params.push(val);
                        }
                    }

                    if this.is_some() {
                        let expr = this.clone().unwrap().clone();
                        let ty = self.get_id_type(expr.id);
                        let val = if !ty.is_ptr() {
                            let cty = self.ty_to_ctype(&ty).make_pointer();
                            let val = self.gen_expr(&Expr {
                                pos: expr.pos,
                                id: expr.id,
                                kind: ExprKind::AddressOf(expr),
                            });
                            let val = self.ctx.new_cast(None, val, cty);
                            val
                        } else {
                            self.gen_expr(&expr)
                        };
                        params.push(val);
                    }

                    return self.ctx.new_call(
                        Some(self.ctx.new_location(
                            str(expr.pos.file).to_string(),
                            expr.pos.line as _,
                            expr.pos.column as _,
                        )),
                        val,
                        &params,
                    );
                } else if self.const_functions.contains_key(&name.name()) {
                    let functions = self.const_functions.get(&name.name()).unwrap().clone();

                    let val = self.search_for_func_const(&param_types, None, &functions);

                    if val.is_none() {
                        print!("Constant function {}(", str(name.name()));
                        for p in param_types.iter() {
                            print!(" {} ", p);
                        }
                        print!(") not found\n");
                        std::process::exit(-1);
                    }

                    return self.ctx.new_rvalue_from_int(self.ctx.new_type::<i32>(), 0);
                } else if self.external_functions.contains_key(&name.name()) {
                    let unit: &FunctionUnit =
                        &self.external_functions.get(&name.name()).unwrap().clone();

                    let mut params = vec![];
                    for (i, arg) in args.iter().enumerate() {
                        if i < unit.f.params.len() {
                            let val = self.gen_expr(arg);
                            let cty = self.ty_to_ctype(&unit.f.params[i].1);
                            let val = if !unit.f.params[i].1.is_struct()
                                && !unit.f.params[i].1.is_array()
                            {
                                self.ctx.new_cast(None, val, cty)
                            } else {
                                val
                            };

                            params.push(val);
                            continue;
                        }
                        let val = self.gen_expr(arg);

                        params.push(val);
                    }
                    return self.ctx.new_call(
                        Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                        unit.c,
                        &params,
                    );
                } else if let Some(var) = self.variables.get(&name.name()) {
                    var.lval
                } else {
                    panic!();
                };

                let mut params = vec![];
                for arg in args.iter() {
                    params.push(self.gen_expr(arg));
                }

                self.ctx.new_call_through_ptr(
                    Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                    var,
                    &params,
                )
            }

            ExprKind::Struct(name, args) => {
                let name = name.name();

                let struct_: GccStruct = self
                    .find_struct(&Type::create_basic(expr.id, expr.pos, name))
                    .expect("Struct not found");
                //let rval: RValue = self.ctx.new_rvalue_zero(struct_.ty.as_type());
                let tmp_ = format!("_{}_", self.tmp_id);
                self.tmp_id += 1;
                let tmp: LValue = self.cur_func.unwrap().new_local(
                    Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                    struct_.ty,
                    &tmp_,
                );
                //self.cur_block.unwrap().add_assignment(None, tmp, rval);
                for arg in args.iter() {
                    let arg: &StructArg = arg;
                    let val = self.gen_expr(&arg.expr);
                    self.cur_block.unwrap().add_assignment(
                        Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                        tmp.access_field(
                            Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                            *struct_.fields.get(&arg.name).unwrap(),
                        ),
                        val,
                    );
                }
                tmp.to_rvalue()
            }
            ExprKind::SizeOf(ty) => {
                let size = self.ty_size(ty);
                self.ctx
                    .new_rvalue_from_int(self.ctx.new_type::<usize>(), size as i32)
            }
            ExprKind::GetFunc(name) => {
                if self.functions.contains_key(name) {
                    let functions: &Vec<FunctionUnit> = self.functions.get(name).unwrap();
                    let mut v = None;
                    for unit in functions.iter() {
                        let unit: &FunctionUnit = unit;

                        if &unit.f.name == name {
                            v = Some(
                                unit.c
                                    .get_address(Some(gccloc_from_loc(&self.ctx, &expr.pos))),
                            );
                            break;
                        }
                    }
                    v.expect("Function addr")
                } else if self.external_functions.contains_key(name) {
                    let func = self.external_functions.get(name).unwrap();

                    func.c
                        .get_address(Some(gccloc_from_loc(&self.ctx, &expr.pos)))
                } else {
                    panic!("Function not found");
                }
            }

            ExprKind::Binary(op, e1, e2) => {
                let t1 = self.get_id_type(e1.id);
                let t2 = self.get_id_type(e2.id);
                use crate::semantic::{ty_is_any_float, ty_is_any_int};
                let op: &str = op;
                if op.contains("==")
                    || op.contains("!=")
                    || op == "<"
                    || op == ">"
                    || op.contains(">=")
                    || op.contains("<=")
                {
                    let op: &str = op;
                    let comparison = match op {
                        "==" => ComparisonOp::Equals,
                        ">" => ComparisonOp::GreaterThan,
                        "<" => ComparisonOp::LessThan,
                        ">=" => ComparisonOp::GreaterThanEquals,
                        "<=" => ComparisonOp::LessThanEquals,
                        "!=" => ComparisonOp::NotEquals,
                        _ => unreachable!(),
                    };
                    let cty = self.ty_to_ctype(&t1);
                    let e1 = self.gen_expr(e1);
                    let r2 = self.gen_expr(e2);
                    let r2 = self
                        .ctx
                        .new_cast(Some(gccloc_from_loc(&self.ctx, &e2.pos)), r2, cty);
                    return self.ctx.new_comparison(
                        Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                        comparison,
                        e1,
                        r2,
                    );
                }

                if t1.is_ptr() && crate::semantic::ty_is_any_int(&t2) {
                    let array = self.gen_expr(e1);
                    let index = self.gen_expr(e2);
                    self.ctx
                        .new_array_access(Some(gccloc_from_loc(&self.ctx, &expr.pos)), array, index)
                        .to_rvalue()
                } else if ty_is_any_int(&t1) && ty_is_any_int(&t2) {
                    let cty = self.ty_to_ctype(&t1);
                    let op: &str = op;
                    let binary = match op {
                        "+" => BinaryOp::Plus,
                        "-" => BinaryOp::Minus,
                        "*" => BinaryOp::Mult,
                        "/" => BinaryOp::Divide,
                        "%" => BinaryOp::Modulo,
                        "|" => BinaryOp::BitwiseAnd,
                        "&" => BinaryOp::BitwiseAnd,
                        "^" => BinaryOp::BitwiseXor,
                        ">>" => BinaryOp::RShift,
                        "<<" => BinaryOp::LShift,

                        _ => panic!("{}", op),
                    };
                    let l = self.gen_expr(e1);
                    let r = self.gen_expr(e2);
                    let r = self
                        .ctx
                        .new_cast(Some(gccloc_from_loc(&self.ctx, &e2.pos)), r, cty);
                    self.ctx.new_binary_op(
                        Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                        binary,
                        cty,
                        l,
                        r,
                    )
                } else if ty_is_any_float(&t1) && ty_is_any_float(&t2) {
                    let cty = self.ty_to_ctype(&t1);
                    let op: &str = op;
                    let binary = match op {
                        "+" => BinaryOp::Plus,
                        "-" => BinaryOp::Minus,
                        "*" => BinaryOp::Mult,
                        "/" => BinaryOp::Divide,
                        "%" => BinaryOp::Modulo,

                        _ => unreachable!(),
                    };
                    let l = self.gen_expr(e1);
                    let r = self.gen_expr(e2);
                    let r = self
                        .ctx
                        .new_cast(Some(gccloc_from_loc(&self.ctx, &e2.pos)), r, cty);
                    self.ctx.new_binary_op(
                        Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                        binary,
                        cty,
                        l,
                        r,
                    )
                } else if ty_is_any_float(&t1) && ty_is_any_int(&t2) {
                    let cty = self.ty_to_ctype(&t1);
                    let op: &str = op;
                    let binary = match op {
                        "+" => BinaryOp::Plus,
                        "-" => BinaryOp::Minus,
                        "*" => BinaryOp::Mult,
                        "/" => BinaryOp::Divide,
                        "%" => BinaryOp::Modulo,

                        _ => unreachable!(),
                    };
                    let l = self.gen_expr(e1);
                    let r = self.gen_expr(e2);
                    let r = self
                        .ctx
                        .new_cast(Some(gccloc_from_loc(&self.ctx, &e2.pos)), r, cty);
                    self.ctx.new_binary_op(
                        Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                        binary,
                        cty,
                        l,
                        r,
                    )
                } else if t1.is_vec() && ty_is_any_int(&t2) || ty_is_any_float(&t2) {
                    let cty = self.ty_to_ctype(&t1);
                    let op: &str = op;
                    let binary = match op {
                        "+" => BinaryOp::Plus,
                        "-" => BinaryOp::Minus,
                        "*" => BinaryOp::Mult,
                        "/" => BinaryOp::Divide,
                        "%" => BinaryOp::Modulo,

                        _ => unreachable!(),
                    };
                    let l = self.gen_expr(e1);
                    let r = self.gen_expr(e2);
                    let r = self
                        .ctx
                        .new_cast(Some(gccloc_from_loc(&self.ctx, &e2.pos)), r, cty);
                    self.ctx.new_binary_op(
                        Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                        binary,
                        cty,
                        l,
                        r,
                    )
                } else {
                    if t1.is_basic() && t2.is_basic() {
                        let t1 = t1.to_basic().unwrap();
                        let t2 = t2.to_basic().unwrap();
                        let s1: &str = &str(t1.name).to_string();
                        let s2: &str = &str(t2.name).to_string();
                        if s1 == "bool" && s2 == "bool" {
                            let op: &str = op;
                            let binary = match op {
                                "&&" => BinaryOp::LogicalAnd,
                                "||" => BinaryOp::LogicalOr,
                                _ => unreachable!(),
                            };
                            let l = self.gen_expr(e1);
                            let r = self.gen_expr(e2);
                            return self.ctx.new_binary_op(
                                Some(gccloc_from_loc(&self.ctx, &expr.pos)),
                                binary,
                                self.ctx.new_type::<bool>(),
                                l,
                                r,
                            );
                        } else {
                            unimplemented!()
                        }
                    }
                    unimplemented!()
                }
            }
            ExprKind::Char(c) => self
                .ctx
                .new_rvalue_from_int(self.ctx.new_type::<char>(), *c as i32),
            ExprKind::Null => self
                .ctx
                .new_rvalue_from_ptr(self.ctx.new_type::<*mut u8>(), 0 as *mut ()),
            v => panic!("{:?}", v),
        }
    }

    pub fn gen_toplevel(&mut self, elems: &mut [Elem]) {
        for elem in elems.iter() {
            match elem {
                Elem::Struct(s) => {
                    let s: &crate::syntax::ast::Struct = s;
                    let mut fields = vec![];
                    let mut cfields = HashMap::new();
                    let mut types = vec![];
                    for field in s.fields.iter() {
                        let field: &StructField = field;
                        let cty = self.ty_to_ctype(&field.data_type).clone();
                        types.push(field.data_type.clone());
                        let name: &str = &str(field.name).to_string();
                        let cfield = self.ctx.new_field(
                            Some(gccloc_from_loc(&self.ctx, &field.pos)),
                            cty,
                            name,
                        );
                        cfields.insert(field.name, cfield);
                        fields.push(cfield);
                    }

                    let struct_ = if s.union {
                        self.ctx.new_union_type(
                            Some(gccloc_from_loc(&self.ctx, &s.pos)),
                            &str(s.name).to_string(),
                            &fields,
                        )
                    } else {
                        self.ctx
                            .new_struct_type(
                                Some(gccloc_from_loc(&self.ctx, &s.pos)),
                                &str(s.name).to_string(),
                                &fields,
                            )
                            .as_type()
                    };

                    let cstruct = GccStruct {
                        ty: struct_,
                        fields: cfields,
                        types,
                    };
                    if !self.structures.contains_key(&s.name) {
                        self.structures.insert(s.name, cstruct);
                    }
                }
                Elem::Link(name) => {
                    self.ctx.add_driver_option(&format!("-l{}", str(*name)));
                }
                Elem::ConstExpr { name, expr, .. } => {
                    self.constants.insert(*name, *expr.clone());
                }
                Elem::Alias(name, ty) => {
                    self.aliases.insert(*name, ty.clone());
                }
                _ => (),
            }
        }

        for elem in elems.iter_mut() {
            match elem {
                Elem::Func(func) => {
                    let func: &mut Function = func;
                    let linkage = if func.external {
                        FunctionType::Extern
                    } else if func.static_ || !func.public {
                        FunctionType::Internal
                    } else if func.inline {
                        FunctionType::AlwaysInline
                    } else {
                        FunctionType::Exported
                    };

                    if func.external || func.internal {
                        let mut params = vec![];

                        for (name, ty) in func.params.iter() {
                            let tyi = self.ty_to_ctype(ty);
                            params.push(self.ctx.new_parameter(
                                Some(gccloc_from_loc(&self.ctx, &ty.pos())),
                                tyi,
                                &str(*name).to_string(),
                            ));
                        }

                        let f = if func.internal {
                            self.ctx.get_builtin_function(&str(func.name).to_string())
                        } else {
                            let ret = self.ty_to_ctype(&func.ret);
                            self.ctx.new_function(
                                None,
                                linkage,
                                ret,
                                &params,
                                &str(func.name).to_string(),
                                func.variadic,
                            )
                        };

                        let unit = FunctionUnit {
                            f: func.clone(),
                            c: f,
                            this_ast: None,
                            this_ir: None,
                            irname: str(func.name).to_string(),
                        };

                        self.external_functions.insert(func.name, unit);
                    } else {
                        let mut params = vec![];

                        for (name, ty) in func.params.iter() {
                            let ty = self.ty_to_ctype(ty);
                            params.push(self.ctx.new_parameter(None, ty, &str(*name).to_string()));
                        }
                        if func.this.is_some() {
                            let (name, ty) = func.this.as_ref().unwrap();
                            let ty = self.ty_to_ctype(ty);
                            params.push(self.ctx.new_parameter(None, ty, &str(*name).to_string()));
                        }
                        let name_str: &str = &str(func.name).to_string();
                        let id = self.fun_id;

                        func.ir_temp_id = id;
                        let name = if name_str == "main" {
                            "main".to_owned()
                        } else {
                            fn ty_to_n(ty: &Type) -> String {
                                let mut s = String::new();

                                match ty {
                                    Type::Vector(v) => {
                                        s.push_str(&format!("vec{}{}", v.subtype, v.size))
                                    }
                                    Type::Basic(b) => s.push_str(&str(b.name)),
                                    Type::Ptr(ptr) => {
                                        s.push_str("ptr");
                                        s.push_str(&ty_to_n(&ptr.subtype));
                                    }
                                    Type::Func(_) => {
                                        s.push_str(&format!("{}", ty));
                                    }
                                    Type::Struct(st) => s.push_str(&format!("{}", str(st.name))),
                                    Type::Void(_) => s.push_str("v"),
                                    Type::Array(array) => {
                                        s.push_str("ptr");
                                        s.push_str(&ty_to_n(&array.subtype));
                                    }
                                }

                                s
                            }
                            let mut name = str(func.name).to_string();
                            if func.this.is_some() {
                                name.push_str("this");
                                let this = *func.this.clone().unwrap().1.clone();
                                name.push_str(&ty_to_n(&this));
                            }
                            for (_, param) in func.params.iter() {
                                name.push_str(&ty_to_n(param));
                            }
                            name
                        };

                        let ret = self.ty_to_ctype(&func.ret);

                        let f = self.ctx.new_function(
                            None,
                            linkage,
                            ret,
                            &params,
                            &name,
                            func.variadic,
                        );

                        let (this_ast, this_ir) = if let Some((_, ty)) = &func.this {
                            let ty = *ty.clone();
                            let irty = self.ty_to_ctype(&ty);

                            (Some(ty), Some(irty))
                        } else {
                            (None, None)
                        };

                        if let Some(functions) = self.functions.get_mut(&func.name) {
                            let mut found = false;
                            for fun in functions.iter_mut() {
                                if fun.f.name == func.name
                                    && fun.f.params == func.params
                                    && fun.f.ret == func.ret
                                    && fun.f.variadic == func.variadic
                                    && fun.f.this == func.this
                                {
                                    *fun = FunctionUnit {
                                        f: func.clone(),
                                        c: f,
                                        this_ast: this_ast.clone(),
                                        this_ir,
                                        irname: name.clone(),
                                    };
                                    self.fun_id += 1;
                                    found = true;

                                    break;
                                }
                            }
                            if !found {
                                functions.push(FunctionUnit {
                                    f: func.clone(),
                                    c: f,
                                    this_ast: this_ast.clone(),
                                    this_ir,
                                    irname: name,
                                });
                            }
                        } else {
                            self.functions.insert(
                                func.name,
                                vec![FunctionUnit {
                                    f: func.clone(),
                                    c: f,
                                    this_ast,
                                    this_ir,
                                    irname: name,
                                }],
                            );
                        }
                        self.fun_id += 1;
                    }
                }
                _ => (),
            }
        }
        for elem in elems.iter() {
            match elem {
                Elem::Global(global) => {
                    let global: &crate::syntax::ast::Global = global;
                    let cty = self.ty_to_ctype(&global.typ);
                    let name: &str = &str(global.name).to_string();
                    let lval = if global.external {
                        self.ctx.new_global(None, GlobalKind::External, cty, name)
                    } else if global.public {
                        self.ctx.new_global(None, GlobalKind::Exported, cty, name)
                    } else {
                        self.ctx.new_global(None, GlobalKind::Internal, cty, name)
                    };

                    let varinfo = VarInfo {
                        lval,
                        cty,
                        ty: *global.typ.clone(),
                    };

                    self.globals
                        .insert(global.name, (varinfo, global.expr.clone()));
                }
                _ => (),
            }
        }
        for elem in elems.iter() {
            match elem {
                Elem::Func(func) => {
                    if func.external || func.internal {
                        continue;
                    } else {
                        let func: &Function = func;

                        let functions = self.functions.get(&func.name).unwrap().clone();

                        for fun in functions.iter() {
                            if fun.f.ir_temp_id == func.ir_temp_id {
                                self.cur_func = Some(fun.c);
                                self.cur_block = Some(fun.c.new_block("entry"));
                                let block = self.cur_block.unwrap();

                                if &str(func.name).to_string() == "main" {
                                    for (_, (varinfo, expr)) in self.globals.clone().iter() {
                                        if expr.is_some() {
                                            let val = self.gen_expr(expr.as_ref().unwrap());
                                            block.add_assignment(None, varinfo.lval, val);
                                        }
                                    }
                                }

                                for (i, (name, param)) in func.params.iter().enumerate() {
                                    let cty = self.ty_to_ctype(param);
                                    let loc = fun.c.new_local(None, cty, &str(*name).to_string());
                                    let param_ = fun.c.get_param(i as _);
                                    block.add_assignment(None, loc, param_.to_rvalue());
                                    self.variables.insert(
                                        *name,
                                        VarInfo {
                                            lval: loc,
                                            cty,
                                            ty: *param.clone(),
                                        },
                                    );
                                }

                                if let Some((name, ty)) = &func.this {
                                    let cty = self.ty_to_ctype(ty);
                                    let loc = fun.c.new_local(None, cty, &str(*name).to_string());
                                    let param_ = fun.c.get_param(func.params.len() as _);
                                    block.add_assignment(None, loc, param_);
                                    self.variables.insert(
                                        *name,
                                        VarInfo {
                                            lval: loc,
                                            cty,
                                            ty: *ty.clone(),
                                        },
                                    );
                                }
                                self.cur_return = Some(*func.ret.clone());
                                self.gen_stmt(func.body.as_ref().unwrap(), true);
                                /*if !self.terminated.last().unwrap_or(&false)
                                {
                                    let ret = self.cur_return.clone().unwrap().clone();
                                    if ret.is_void()
                                    {
                                        self.cur_block.unwrap().end_with_void_return(None);
                                    }
                                    else
                                    {
                                        if ret.is_struct()
                                        {
                                            panic!("Can't create zero value for struct");
                                        }
                                        if !self.terminated.last().unwrap_or(&false)
                                        {
                                            let val =
                                                self.ctx.new_rvalue_zero(self.ty_to_ctype(&ret));
                                            self.cur_block.unwrap().end_with_return(None, val);
                                        }
                                    }
                                }*/
                                //let cty = ty_to_ctype(&func.ret, &self.ctx);
                                //block.end_with_return(None,self.ctx.new_rvalue_zero(cty));
                            }
                        }
                    }
                }

                _ => (),
            }
        }
    }

    pub fn compile(&mut self) {
        if self.context.emit_asm && self.context.jit {
            self.ctx.set_dump_code(true);
        }

        self.ctx
            .set_opt_level(unsafe { std::mem::transmute(i32::from(self.context.opt)) });

        let mut elems = self.context.file.elems.clone();

        self.gen_toplevel(&mut elems);

        if self.context.jit {
            use std::env::args;

            let result = self.ctx.compile();
            let args = args();
            let argc = args.len() as i32;
            let argv: Vec<String> = args.collect::<Vec<String>>();
            let argv_c = argv
                .iter()
                .map(|s| std::ffi::CString::new(s.as_bytes()).unwrap().as_ptr())
                .collect::<Vec<_>>();

            let env = std::env::vars();
            let mut envp = vec![];
            for (key, val) in env {
                envp.push(CString::new(format!("{} = {}", key, val)).unwrap().as_ptr());
            }

            let main_fn: fn(i32, *const *const i8, *const *const i8) -> i32 =
                unsafe { std::mem::transmute(result.get_function("main")) };

            main_fn(argc, argv_c.as_ptr(), envp.as_slice().as_ptr());
        } else {
            // these two calls needed because by default binary don't linked with libc and
            // libm
            self.ctx.add_driver_option("-lc"); // link libc
            self.ctx.add_driver_option("-lm"); // link libm
            let out_path = if !self.context.output.is_empty() {
                self.context.output.clone()
            } else {
                "a.out".to_owned()
            };
            let kind = if self.context.emit_obj {
                OutputKind::ObjectFile
            } else if self.context.shared {
                OutputKind::DynamicLibrary
            } else if self.context.emit_asm {
                OutputKind::Assembler
            } else {
                OutputKind::Executable
            };
            self.ctx.compile_to_file(kind, out_path);
        }
    }
}
