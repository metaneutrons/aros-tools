#include <assert.h>
#include <stddef.h>
#include <string.h>

#include <hardware/smbios.h>

_Static_assert(offsetof(struct SMBIOSEntryPoint3, table_length) == 0x0c,
    "SMBIOS 3 table length has the specification offset");
_Static_assert(offsetof(struct SMBIOSEntryPoint3, table_address) == 0x10,
    "SMBIOS 3 table address has the specification offset");

static void set_checksum(UBYTE *entry, size_t length, size_t checksum_offset)
{
    UBYTE checksum = 0;
    size_t i;

    entry[checksum_offset] = 0;
    for (i = 0; i < length; i++)
        checksum += entry[i];
    entry[checksum_offset] = (UBYTE)(0 - checksum);
}

int main(void)
{
    UBYTE firmware_text[64] = "_SM3_\0etc/extra-pci-roots";
    UBYTE smbios2[0x1f] = {0};
    UBYTE smbios3[0x18] = {0};

    assert(!SMBIOS_EntryPointValid(firmware_text, 3,
        firmware_text + sizeof(firmware_text)));

    memcpy(smbios3, "_SM3_", 5);
    smbios3[6] = sizeof(smbios3);
    set_checksum(smbios3, sizeof(smbios3), 5);
    assert(SMBIOS_EntryPointValid(smbios3, 3,
        smbios3 + sizeof(smbios3)));
    assert(!SMBIOS_EntryPointValid(smbios3, 3, smbios3 + 7));
    smbios3[7]++;
    assert(!SMBIOS_EntryPointValid(smbios3, 3,
        smbios3 + sizeof(smbios3)));

    memcpy(smbios2, "_SM_", 4);
    smbios2[5] = sizeof(smbios2);
    memcpy(smbios2 + 0x10, "_DMI_", 5);
    set_checksum(smbios2 + 0x10, 0x0f, 5);
    set_checksum(smbios2, sizeof(smbios2), 4);
    assert(SMBIOS_EntryPointValid(smbios2, 2,
        smbios2 + sizeof(smbios2)));
    smbios2[0x15]++;
    set_checksum(smbios2, sizeof(smbios2), 4);
    assert(!SMBIOS_EntryPointValid(smbios2, 2,
        smbios2 + sizeof(smbios2)));

    return 0;
}
