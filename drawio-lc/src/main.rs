mod model;
mod transform;

use std::{env, fs, process};

use model::Config;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <config.yaml>", args[0]);
        process::exit(1);
    }

    let content = fs::read_to_string(&args[1]).unwrap_or_else(|e| {
        eprintln!("Error reading {}: {}", args[1], e);
        process::exit(1);
    });

    let config: Config = serde_yaml::from_str(&content).unwrap_or_else(|e| {
        eprintln!("Error parsing YAML: {}", e);
        process::exit(1);
    });

    let input_xml = fs::read_to_string(&config.original).unwrap_or_else(|e| {
        eprintln!("Error reading original file {}: {}", config.original, e);
        process::exit(1);
    });

    for derived in &config.derived {
        let from_xml = if derived.from == config.original {
            input_xml.clone()
        } else {
            fs::read_to_string(&derived.from).unwrap_or_else(|e| {
                eprintln!("Error reading {}: {}", derived.from, e);
                process::exit(1);
            })
        };

        transform::transform(&from_xml, derived).unwrap_or_else(|e| {
            eprintln!("Error transforming into {}: {}", derived.output, e);
            process::exit(1);
        });

        println!("Generated {}", derived.output);
    }
}
