/* Load platform declarations first: C23 hosts may define generic string macros. */
#include <stddef.h>
#include <string.h>

void *hull_quickjs_native_memchr(const void *, int, size_t);
char *hull_quickjs_native_strchr(const char *, int);
char *hull_quickjs_native_strrchr(const char *, int);
int hull_quickjs_native_strcmp(const char *, const char *);

#undef memchr
#undef strchr
#undef strrchr
#undef strcmp
#define memchr hull_quickjs_native_memchr
#define strchr hull_quickjs_native_strchr
#define strrchr hull_quickjs_native_strrchr
#define strcmp hull_quickjs_native_strcmp
