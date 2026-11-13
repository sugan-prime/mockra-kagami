.global _start
.text

_start:
    // 1. Setup Stack Pointer at 0x80008000
    movz x0, #0x8000, lsl #16
    movk x0, #0x8000
    mov sp, x0

    // 2. Print initial banner via UART (0xD0000000)
    adr x0, msg_hello
    movz x1, #0xD000, lsl #16
    bl print_string

    // 3. Compute sum of 1..10
    movz x2, #10        // loop counter
    movz x3, #0         // sum
sum_loop:
    add x3, x3, x2
    subs x2, x2, #1
    b.ne sum_loop

    // 4. Verify sum == 55
    cmp x3, #55
    b.eq test_passed

test_failed:
    adr x0, msg_fail
    movz x1, #0xD000, lsl #16
    bl print_string
    svc #1              // Exit with failure code

test_passed:
    adr x0, msg_pass
    movz x1, #0xD000, lsl #16
    bl print_string
    svc #0              // Exit with success code

// ----------------------------------------------------------------------------
// print_string:
// Inputs:
//   X0: Pointer to null-terminated ASCII string
//   X1: MMIO UART base address (0xD0000000)
// Uses:
//   W4: character byte
// ----------------------------------------------------------------------------
print_string:
    ldrb w4, [x0], #1
    cbz w4, print_done
    strb w4, [x1, #0]
    b print_string
print_done:
    ret

// ----------------------------------------------------------------------------
// Read-Only String Literals
// ----------------------------------------------------------------------------
.align 4
msg_hello:
    .asciz "Hello from ARM64 Bare-Metal ROM!\n"
msg_pass:
    .asciz "Test Passed: Sum(1..10) == 55\n"
msg_fail:
    .asciz "Test Failed!\n"
