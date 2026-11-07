/* QuickJS-owned byte/string primitives. No allocator or kernel services. */
#include <stddef.h>

void *hull_quickjs_native_memchr(const void *bytes, int value, size_t length) {
    const unsigned char *cursor = bytes;
    const unsigned char needle = (unsigned char)value;
    for (size_t index = 0; index < length; index++) {
        if (cursor[index] == needle) return (void *)(cursor + index);
    }
    return NULL;
}

char *hull_quickjs_native_strchr(const char *text, int value) {
    const unsigned char *cursor = (const unsigned char *)text;
    const unsigned char needle = (unsigned char)value;
    do {
        if (*cursor == needle) return (char *)cursor;
    } while (*cursor++ != 0);
    return NULL;
}

char *hull_quickjs_native_strrchr(const char *text, int value) {
    const unsigned char *cursor = (const unsigned char *)text;
    const unsigned char needle = (unsigned char)value;
    const unsigned char *last = NULL;
    do {
        if (*cursor == needle) last = cursor;
    } while (*cursor++ != 0);
    return (char *)last;
}

int hull_quickjs_native_strcmp(const char *left, const char *right) {
    const unsigned char *a = (const unsigned char *)left;
    const unsigned char *b = (const unsigned char *)right;
    while (*a != 0 && *a == *b) { a++; b++; }
    return (int)*a - (int)*b;
}
