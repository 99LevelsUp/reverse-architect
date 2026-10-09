//! A test component for the plugin host (U7).
//!
//! Each input path exercises one boundary the host has to hold: a guest that
//! tries to read the disk, open a socket, read the environment, read the clock,
//! draw randomness, trap, spin forever or allocate without bound. The result of
//! each attempt comes back as the signature of one symbol, so a host test can
//! assert on what the guest actually saw.
//!
//! Not a language plugin anyone would load: its only language is `.probe`.

wit_bindgen::generate!({
    path: "../../wit",
    world: "language-plugin",
});

use exports::ra::plugin::extractor::Guest;
use ra::plugin::host::path_exists;
use ra::plugin::types::{
    Config, Fact, Language, Manifest, Op, Place, Symbol, Usage, UsageKind, Value, Vocabulary,
};
use std::collections::HashMap;

struct Probe;

fn symbol(path: &str, signature: String) -> Op {
    let fact = |kind: &str, value: Value| Fact {
        kind: kind.into(),
        value,
        place: Place::Range((1, 1)),
    };
    Op::Symbol(Symbol {
        key: format!("sym:probe:{path}#Thing"),
        facts: vec![
            fact("symbol_name", Value::Text("Thing".into())),
            fact("symbol_kind", Value::Text("class".into())),
            fact("symbol_signature", Value::Text(signature)),
        ],
        edges: Vec::new(),
    })
}

fn attempt(path: &str, config: &Config) -> String {
    match path {
        "trap.probe" => panic!("the probe traps on purpose"),
        "loop.probe" => {
            let mut n: u64 = 0;
            loop {
                n = n.wrapping_add(1);
                std::hint::black_box(n);
            }
        }
        "memory.probe" => {
            let big: Vec<u8> = vec![7; 1 << 30];
            format!("allocated {}", big.len())
        }
        "fs.probe" => match std::fs::read_dir("/") {
            Ok(_) => "fs:open".into(),
            Err(_) => "fs:refused".into(),
        },
        "net.probe" => match std::net::TcpStream::connect("127.0.0.1:80") {
            Ok(_) => "net:connected".into(),
            Err(_) => "net:refused".into(),
        },
        "env.probe" => format!(
            "env:{} args:{}",
            std::env::vars().count(),
            std::env::args().count()
        ),
        "clock.probe" => {
            let wall = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("clock:{wall}")
        }
        "hash.probe" => {
            let mut m: HashMap<&str, u32> = HashMap::new();
            for (i, k) in ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"]
                .iter()
                .enumerate()
            {
                m.insert(k, i as u32);
            }
            m.keys().copied().collect::<Vec<_>>().join(",")
        }
        "random.probe" => {
            let mut buf = [0u8; 16];
            match getrandom::fill(&mut buf) {
                Ok(()) => buf.iter().map(|b| format!("{b:02x}")).collect(),
                Err(_) => "random:unavailable".into(),
            }
        }
        "stdout.probe" => {
            println!("a plugin must not write into host output");
            eprintln!("nor into its error stream");
            "stdout:written".into()
        }
        "path.probe" => format!(
            "exists:{} missing:{}",
            path_exists("src/a.rs"),
            path_exists("no/such/file")
        ),
        _ => format!("prefixes:{}", config.secret_prefixes.len()),
    }
}

impl Guest for Probe {
    fn describe() -> Manifest {
        Manifest {
            id: "probe".into(),
            version: "0.1.0".into(),
            languages: vec![Language {
                name: "probe".into(),
                extensions: vec!["probe".into()],
            }],
            trust_rank: 1,
            depends_on: Vec::new(),
        }
    }

    fn declare() -> Option<Vocabulary> {
        None
    }

    fn declarations(path: String, _source: Vec<u8>, config: Config) -> Vec<Op> {
        let result = attempt(&path, &config);
        vec![symbol(&path, result)]
    }

    fn usages(_path: String, _source: Vec<u8>) -> Vec<Usage> {
        vec![Usage {
            name: "Thing".into(),
            kind: UsageKind::Call,
            line: 1,
        }]
    }
}

export!(Probe);
