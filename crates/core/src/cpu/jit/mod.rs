//! High-Performance Dynamic Binary Translation (JIT) Subsystem.
//!
//! Provides basic block compilation, micro-op intermediate representation,
//! block caching, chaining, and an accelerated execution engine.

pub mod block;
pub mod cache;
pub mod compiler;
pub mod engine;

pub use block::{BasicBlock, BlockOutcome, BlockTerminal, MicroOp};
pub use cache::{BlockCache, CacheStats};
pub use compiler::{BlockCompiler, CompileError, MAX_BLOCK_INSTRUCTIONS};
pub use engine::{EngineMetrics, JitEngine, JitError};
