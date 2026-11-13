SECTIONS
{
    . = 0x0;
    .text : {
        LONG(0x14000020) /* b 0x80 */
        LONG(0x00000180) /* mod0 offset = 0x180 */
        . = 0x80;
        *(.text*)
    }
    . = 0x200;
    .rodata : {
        *(.rodata*)
    }
    . = 0x300;
    .data : {
        *(.data*)
    }
}
