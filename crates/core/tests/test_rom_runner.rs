use emu_core::cpu::{Cpu, StepResult};
use emu_core::loader::load_flat_binary;
use emu_core::memory::SystemBus;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// A custom test UART that records output into a shared buffer for assertion.
#[derive(Clone, Default)]
struct TestUart {
    captured: Arc<Mutex<Vec<u8>>>,
}

impl emu_core::memory::MmioDevice for TestUart {
    fn read(&self, _offset: u64, _size: usize) -> Result<u64, emu_core::memory::MemoryError> {
        Ok(1) // Ready
    }

    fn write(&mut self, offset: u64, val: u64, _size: usize) -> Result<(), emu_core::memory::MemoryError> {
        if offset == 0x00 {
            self.captured.lock().unwrap().push(val as u8);
        }
        Ok(())
    }
}

#[test]
fn test_bare_metal_hello_uart_rom() {
    let mut bus = SystemBus::new();

    // Map 64KB RAM at 0x8000_0000
    const RAM_BASE: u64 = 0x8000_0000;
    const RAM_SIZE: usize = 0x10000;
    bus.map_ram(RAM_BASE, RAM_SIZE);

    // Map UART device at 0xD000_0000
    const UART_BASE: u64 = 0xD000_0000;
    const UART_SIZE: u64 = 0x1000;
    let test_uart = TestUart::default();
    let captured_log = test_uart.captured.clone();
    bus.map_mmio(UART_BASE, UART_SIZE, Box::new(test_uart));

    // Load assembled bare-metal ROM
    let rom_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/test_roms/hello_uart.bin");

    let entry_point = load_flat_binary(&mut bus, &rom_path, RAM_BASE)
        .expect("Failed to load hello_uart.bin");
    assert_eq!(entry_point, RAM_BASE);

    let mut cpu = Cpu::new();
    cpu.reset(entry_point);

    // Step CPU until SVC or limit
    let mut cycles = 0;
    const MAX_CYCLES: u64 = 10_000;
    let mut exit_code = None;

    while cycles < MAX_CYCLES {
        cycles += 1;
        match cpu.step(&mut bus).expect("CPU execution error") {
            StepResult::Continue => continue,
            StepResult::Svc(code) => {
                exit_code = Some(code);
                break;
            }
            StepResult::Halt | StepResult::Breakpoint => break,
        }
    }

    // Verify exit code is 0 (test_passed)
    assert_eq!(exit_code, Some(0), "ROM did not exit with success code (SVC #0)");

    // Verify the exact string captured by UART
    let output = String::from_utf8_lossy(&captured_log.lock().unwrap()).to_string();
    println!("Captured UART Output:\n{}", output);

    assert!(output.contains("Hello from ARM64 Bare-Metal ROM!"));
    assert!(output.contains("Test Passed: Sum(1..10) == 55"));
}
