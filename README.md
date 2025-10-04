# MelisDB

[![Rust](https://github.com/laohanlinux/melisdb/workflows/Rust/badge.svg)](https://github.com/laohanlinux/melisdb/actions)
[![Crates.io](https://img.shields.io/crates/v/melisdb.svg)](https://crates.io/crates/melisdb)
[![Documentation](https://docs.rs/melisdb/badge.svg)](https://docs.rs/melisdb)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

MelisDB is a high-performance, embedded key-value database written in Rust. It implements the WiscKey design principles and provides a BadgerDB-compatible API, making it a drop-in replacement for applications using BadgerDB.

## 🚀 Features

- **High Performance**: Optimized for write-heavy workloads with fast random reads
- **LSM-Tree Architecture**: Based on the proven WiscKey design from University of Wisconsin
- **BadgerDB Compatible**: Drop-in replacement for existing BadgerDB applications
- **Memory Efficient**: Value log separation reduces memory usage
- **Concurrent Safe**: Built with Rust's memory safety guarantees
- **CLI Tools**: Complete command-line interface for database management
- **Streaming Support**: Efficient data streaming and backup capabilities

## 📖 Background & Origins

### The WiscKey Foundation

MelisDB is built upon the research and design principles from the [WiscKey paper](https://www.usenix.org/system/files/conference/fast16/fast16-papers-lu.pdf) by researchers at the University of Wisconsin, Madison. This groundbreaking work introduced the concept of separating keys and values in LSM-trees, dramatically improving write performance while maintaining read efficiency.

### The BadgerDB Legacy

The original [BadgerDB](https://github.com/dgraph-io/badger) project, developed by the Dgraph team, was one of the first production implementations of the WiscKey design in Go. BadgerDB demonstrated the practical viability of the WiscKey approach and became widely adopted in the Go ecosystem for high-performance key-value storage needs.

### The badger-rs Contribution

[badger-rs](https://github.com/laohanlinux/badger-rs) was an ambitious Rust port of BadgerDB that aimed to bring the performance and design benefits of BadgerDB to the Rust ecosystem. This project laid the groundwork for understanding how to translate BadgerDB's Go implementation into idiomatic Rust code while maintaining compatibility and performance characteristics.

### MelisDB: The Evolution

MelisDB represents the next evolution of this lineage, building upon the solid foundation of badger-rs while incorporating lessons learned and improvements. The name "Melis" reflects the project's focus on being a "honey" of a database - sweet, efficient, and valuable.

## 🏗️ Architecture

MelisDB follows the WiscKey architecture with these key components:

- **LSM-Tree**: Manages keys and metadata in sorted tables
- **Value Log**: Stores large values separately for better performance
- **MemTable**: In-memory structure for fast writes
- **Compaction**: Background process to maintain performance
- **Transaction Support**: ACID-compliant operations

## 🛠️ Installation

Add MelisDB to your `Cargo.toml`:

```toml
[dependencies]
melisdb = "0.1.0"
```

## 📚 Quick Start

### Basic Usage

```rust
use melisdb::{Options, KV};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open the database
    let opts = Options::default();
    let kv = KV::open(opts).await?;
    
    // Write data
    kv.set(b"key1".to_vec(), b"value1".to_vec(), 0).await?;
    
    // Read data
    let value = kv.get(b"key1").await?;
    println!("Value: {}", String::from_utf8_lossy(&value));
    
    Ok(())
}
```

### CLI Usage

MelisDB includes a comprehensive CLI tool called `melis`:

```bash
# Install the CLI
cargo install melisdb --bin melis

# Run bank test (Jepsen-inspired consistency test)
melis bank test --dir /tmp/test_db --duration 30s --accounts 100

# Get database information
melis info --dir /tmp/test_db

# Stream data between databases
melis stream --source /tmp/source_db --dest /tmp/dest_db
```

## 🧪 Testing

MelisDB includes a comprehensive test suite, including:

- **Bank Test**: A Jepsen-inspired transactional consistency test
- **Unit Tests**: Comprehensive coverage of all components
- **Integration Tests**: End-to-end functionality verification
- **Performance Benchmarks**: Comparison with other databases

Run the test suite:

```bash
cargo test
cargo test --release  # For performance tests
```

## 📊 Performance

MelisDB is designed for high performance, especially in write-heavy scenarios:

- **Write Performance**: Optimized for sequential writes to value log
- **Read Performance**: Fast random reads with efficient key lookup
- **Memory Usage**: Reduced memory footprint through value separation
- **Concurrency**: Excellent multi-threaded performance

## 🤝 Contributing

We welcome contributions to MelisDB! Please see our [Contributing Guide](CONTRIBUTING.md) for details on how to get started.

### Development Setup

```bash
git clone https://github.com/rhize-labs/melisdb.git
cd melisdb
cargo build
cargo test
```

## 📄 License

MelisDB is licensed under the MIT License. See [LICENSE](LICENSE) for details.

## 🙏 Acknowledgments

MelisDB stands on the shoulders of giants and acknowledges the following contributions:

### Research Foundation
- **University of Wisconsin, Madison**: For the groundbreaking WiscKey research that forms the theoretical foundation of this database
- **Lanyue Lu, Thanumalayan Sankaranarayana Pillai, Andrea C. Arpaci-Dusseau, Remzi H. Arpaci-Dusseau**: Authors of the original WiscKey paper

### Implementation Heritage
- **Dgraph Team**: For creating the original [BadgerDB](https://github.com/dgraph-io/badger) in Go, which proved the practical viability of the WiscKey design
- **badger-rs Contributors**: For the initial Rust port that demonstrated how to translate BadgerDB's concepts into idiomatic Rust code
- **Rust Community**: For the excellent ecosystem and tooling that makes projects like MelisDB possible

### Special Thanks
- The open-source community for continuous feedback and contributions
- All users who have tested and provided feedback on early versions
- Contributors to related projects that have influenced MelisDB's design

## 🔗 Related Projects

- [BadgerDB](https://github.com/dgraph-io/badger) - Original Go implementation
- [badger-rs](https://github.com/laohanlinux/badger-rs) - Rust port that inspired MelisDB
- [WiscKey Paper](https://www.usenix.org/system/files/conference/fast16/fast16-papers-lu.pdf) - Original research paper

## 📞 Support

- **Issues**: [GitHub Issues](https://github.com/laohanlinux/melisdb/issues)
- **Discussions**: [GitHub Discussions](https://github.com/laohanlinux/melisdb/discussions)
- **Documentation**: [docs.rs/melisdb](https://docs.rs/melisdb)

---

*MelisDB: Sweet, efficient, and valuable key-value storage for Rust applications.*