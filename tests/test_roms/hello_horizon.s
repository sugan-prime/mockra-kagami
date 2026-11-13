.global main
.text
main:
    // 1. Output string via svcOutputDebugString (SVC 0x27)
    adr x0, msg_horizon
    movz x1, #27
    svc #0x27

    // 2. Allocate heap via svcSetHeapSize (SVC 0x35)
    movz x1, #0x20, lsl #16   // 2MB heap
    svc #0x35

    // Write a test value into the heap base (returned in X1)
    movz x2, #0xbeef
    str w2, [x1, #0]

    // 3. Exit process via svcExitProcess (SVC 0x07)
    svc #0x07

.section .rodata
.align 4
msg_horizon:
    .ascii "Hello Horizon OS from NRO!\n"
