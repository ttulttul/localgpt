// Compatibility shim for glibc < 2.38
// The pre-built ONNX Runtime binary (via ort-sys) references __isoc23_strtol*
// symbols that only exist in glibc 2.38+. These are just C23 wrappers around
// the standard strto* functions with identical behavior for our use case.

#include <stdlib.h>

long __isoc23_strtol(const char *nptr, char **endptr, int base) {
    return strtol(nptr, endptr, base);
}

long long __isoc23_strtoll(const char *nptr, char **endptr, int base) {
    return strtoll(nptr, endptr, base);
}

unsigned long long __isoc23_strtoull(const char *nptr, char **endptr, int base) {
    return strtoull(nptr, endptr, base);
}
