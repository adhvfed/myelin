fn main() {
    eprintln!(
        "myelin-mcp: the standalone database host has been retired; run \
         `myelin mcp serve --as <agent-id>` so Edge owns identity and governance"
    );
    std::process::exit(2);
}
