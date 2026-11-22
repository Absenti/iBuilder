/* mem_probe.c -- prints what the memory / address-space / entitlement functions of the ABI return in the process it runs in (a Mac, the iOS simulator, a device).
 *   xcrun --sdk iphonesimulator clang -arch arm64 -target arm64-apple-ios14.0-simulator -Icrates/ibuilder-ffi/include mem_probe.c libibuilder_ffi.a -framework CoreFoundation -o mem_probe
 *   xcrun simctl spawn <booted sim udid> ./mem_probe <a writable folder>
 */
#include "ibuilder.h"
#include <stdio.h>
#include <stdlib.h>
#include <dlfcn.h>

int main(int argc, char **argv)
{
    const char *docs = argc > 1 ? argv[1] : ".";
    ibuilder_mem m;
    if (ibuilder_mem_info(docs, &m) != 0) { puts("ibuilder_mem_info failed"); return 1; }
    printf("mem_info: resident %.1f MiB, peak %.1f MiB, footprint %.1f MiB, available %llu bytes, free %.2f GiB\n",
           m.resident / 1048576.0, m.peak / 1048576.0, m.footprint / 1048576.0, (unsigned long long) m.available, m.free_bytes / 1073741824.0);
    /* allocate and touch 256 MiB: the footprint and the peak must follow it */
    size_t n = 256u << 20;
    volatile char *p = malloc(n);
    for (size_t i = 0; i < n; i += 4096) p[i] = 1;
    ibuilder_mem m2;
    ibuilder_mem_info(docs, &m2);
    printf("after touching 256 MiB: resident %.1f MiB (+%.1f), peak %.1f MiB, footprint %.1f MiB (+%.1f)\n",
           m2.resident / 1048576.0, (m2.resident - m.resident) / 1048576.0, m2.peak / 1048576.0, m2.footprint / 1048576.0, ((double) m2.footprint - (double) m.footprint) / 1048576.0);
    free((void *) p);
    char line[256];
    ibuilder_mem_line(docs, line, sizeof line);
    printf("mem_line: %s\n", line);
    printf("dlsym(os_proc_available_memory): %s\n", dlsym(RTLD_DEFAULT, "os_proc_available_memory") ? "present" : "absent");
    printf("available_bytes: %lld (-1 = no jetsam ceiling here)\n", (long long) ibuilder_available_bytes());
    ibuilder_address_space a;
    int rc = ibuilder_address_space_info(&a);
    printf("address_space: rc %d, free %.2f GB, largest hole %.2f GB, extended %d, increased_memory %d, get_task_allow %d\n", rc, a.free_total / 1073741824.0, a.largest / 1073741824.0, a.extended, a.increased_memory, a.get_task_allow);
    char id[256];
    ibuilder_self_identifier(id, sizeof id);
    printf("self_identifier: %s\n", id);
    return 0;
}
