#![feature(box_syntax)]

extern crate havo;
extern crate structopt;

use havo::{
    err::MsgWithPos,
    gccjit::Codegen,
    optimize::const_eval,
    semantic::*,
    syntax::{ast::*, lexer::reader::Reader, parser::*},
    Context,
};
use structopt::StructOpt;

use std::path::PathBuf;

#[derive(Debug, StructOpt)]
pub enum Backend {
    #[structopt(help = "Default backend, allows JIT and AOT compilation")]
    GccJIT,
    #[structopt(help = "C++ backend,still W.I.P")]
    CPP,
    #[structopt(help = "Cranelift backend (UNIMPLEMENTED!)")]
    CraneLift,
}

impl Backend {
    pub const fn gccjit() -> &'static str {
        "gccjit"
    }

    pub const fn cpp() -> &'static str {
        "cpp"
    }

    pub const fn cranelift() -> &'static str {
        "cranelift"
    }
}

use std::str::FromStr;

impl FromStr for Backend {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Backend, &'static str> {
        let s: &str = &s.to_lowercase();
        match s {
            "gccjit" => Ok(Backend::GccJIT),
            "cranelift" => Ok(Backend::CraneLift),
            "cpp" | "c++" => Ok(Backend::CPP),
            _ => Err("expected gccjit,cpp or cranelift backend"),
        }
    }
}

#[derive(StructOpt, Debug)]
#[structopt(name = "havo", about = "Havo kompilyatori")]
pub struct Options {
    #[structopt(parse(from_os_str))]
    pub file: PathBuf,
    #[structopt(
        short = "O",
        long = "opt-level",
        default_value = "2",
        help = "Set optimization level"
    )]
    pub opt_level: u8,
    #[structopt(long = "jit", help = "Use JIT compilation instead of AOT compilation")]
    pub jit: bool,
    #[structopt(long = "emit-obj", help = "Output object file")]
    pub emit_obj: bool,
    #[structopt(long = "emit-asm", help = "Print assembly to stdout")]
    pub emit_asm: bool,
    #[structopt(
        short = "o",
        long = "output",
        parse(from_os_str),
        help = "Set output filename"
    )]
    pub output: Option<PathBuf>,
    #[structopt(long = "shared", help = "Output shared library (.dll or .so)")]
    pub shared: bool,
    #[structopt(
        long = "emit-gimple",
        help = "Dump GIMPLE to stdout if gccjit backend used"
    )]
    pub emit_gimple: bool,
    #[structopt(
        long = "backend",
        raw(
            possible_values = "&[\"gccjit\",\"cranelift\",\"cpp\"]",
            case_insensitive = "true",
            default_value = "\"gccjit\""
        ),
        help = "Select backend"
    )]
    pub backend: Backend,
    #[structopt(short = "l", long = "link")]
    pub libraries_link: Vec<String>,
    #[structopt(short = "f")]
    pub gcc_opts: Vec<String>,
    #[structopt(
        long = "consteval",
        help = "Enables constant folding and const function evaluating"
    )]
    pub const_eval: bool,
    #[structopt(long = "print-ast", help = "Print program")]
    pub print_ast: bool,
    #[structopt(
        long = "aggressive-eval",
        help = "try to evaluate normal (not constexpr) functions too"
    )]
    pub aggressive_eval: bool,
}

fn main() -> Result<(), MsgWithPos> {
    let opts: Options = Options::from_args();
    let mut file = File {
        root: opts
            .file
            .parent()
            .unwrap_or(&std::path::Path::new(""))
            .to_str()
            .unwrap()
            .to_owned(),
        src: String::new(),
        path: opts.file.to_str().unwrap().to_owned(),
        elems: vec![],
    };

    let reader = Reader::from_file(opts.file.to_str().unwrap()).unwrap();

    let mut parser = Parser::new(reader, &mut file);

    let err = parser.parse();
    if err.is_err() {
        println!("{}", err.clone().err().unwrap());
        std::process::exit(-1);
    }

    let mut ctx = Context::new(file);
    ctx.shared = opts.shared;
    ctx.emit_asm = opts.emit_asm;
    ctx.emit_obj = opts.emit_obj;
    ctx.jit = opts.jit;
    ctx.output = opts
        .output
        .map_or(String::new(), |e: PathBuf| e.to_str().unwrap().to_owned());
    ctx.opt = opts.opt_level;
    ctx.gimple = opts.emit_gimple;
    ctx.file.elems.extend(
        opts.libraries_link
            .iter()
            .map(|name| havo::ast::Elem::Link(havo::intern(name))),
    );
    let mut semantic = SemCheck::new(&mut ctx);

    semantic.run();
    use havo::eval::EvalCtx;
    /*let mut eval = EvalCtx::new(&mut ctx);
    eval.run();*/
    if opts.print_ast {
        for elem in ctx.file.elems.iter() {
            println!("{}", elem);
        }
    }

    match opts.backend {
        Backend::CPP => {
            use havo::ast2cpp::Translator;
            let mut translator = Translator::new(ctx);
            translator.run();
        }
        Backend::GccJIT => {
            let mut cgen = Codegen::new(&mut ctx, "HavoModule");
            for opt in opts.gcc_opts.iter() {
                cgen.ctx.add_command_line_option(opt);
            }
            cgen.compile();
        }
        Backend::CraneLift => {
            eprintln!("Cranelift backend still unimplemented");
        }
    }

    Ok(())
}
