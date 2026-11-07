/* QuickJS thread primitives. Desktop builds retain their pthread backend;
   TRUEOS stores native mutex/condition state directly in the owning C object. */
#ifndef QUICKJS_THREAD_H
#define QUICKJS_THREAD_H
#include <stdint.h>

#ifdef HULL_QUICKJS_NATIVE_WAIT
#include <stdatomic.h>
enum { JS_OS_WAIT_TIMEOUT = 1 };

extern uint32_t hull_quickjs_native_wait_observe(uint64_t key);
extern void hull_quickjs_native_wait_after(uint64_t key, uint32_t observed,
                                          uint64_t timeout_ms);
extern void hull_quickjs_native_wake_one(uint64_t key);
extern uint64_t hull_quickjs_native_monotonic_nanos(void);

typedef struct { _Atomic(uint32_t) state; } JSOSMutex;
typedef struct { _Atomic(uint32_t) sequence; } JSOSCond;
#define JS_OS_MUTEX_INITIALIZER { 0 }

static void js_os_mutex_lock(JSOSMutex *mutex)
{
    uint32_t expected = 0, observed;
    uint64_t key = (uintptr_t)mutex;
    if (atomic_compare_exchange_strong_explicit(&mutex->state, &expected, 1,
                                                memory_order_acquire,
                                                memory_order_relaxed))
        return;
    for (;;) {
        /* Observe before the last predicate check, so an unlock between the
           exchange and park cannot be lost. 2 preserves the wake obligation. */
        observed = hull_quickjs_native_wait_observe(key);
        if (atomic_exchange_explicit(&mutex->state, 2,
                                     memory_order_acquire) == 0)
            return;
        hull_quickjs_native_wait_after(key, observed, UINT64_MAX);
    }
}

static void js_os_mutex_unlock(JSOSMutex *mutex)
{
    if (atomic_exchange_explicit(&mutex->state, 0,
                                 memory_order_release) == 2)
        hull_quickjs_native_wake_one((uintptr_t)mutex);
}

static void js_os_cond_init(JSOSCond *cond)
{
    atomic_init(&cond->sequence, 0);
}

static void js_os_cond_destroy(JSOSCond *cond)
{
    /* The owning QuickJS waiter has already left its wait and list. */
    (void)cond;
}

static void js_os_cond_signal(JSOSCond *cond)
{
    /* QuickJS allocates one condition per waiter, and signals under the same
       global atomics mutex that protects waiter registration and removal. */
    atomic_fetch_add_explicit(&cond->sequence, 1, memory_order_release);
    hull_quickjs_native_wake_one((uintptr_t)cond);
}

static int js_os_cond_timedwait(JSOSCond *cond, JSOSMutex *mutex,
                                int64_t timeout_ms)
{
    uint64_t key = (uintptr_t)cond;
    uint64_t start;
    uint64_t remaining, elapsed;
    uint32_t sequence = atomic_load_explicit(&cond->sequence,
                                             memory_order_acquire);
    uint32_t observed;
    int result = 0;
    if (timeout_ms == 0)
        return JS_OS_WAIT_TIMEOUT;
    start = hull_quickjs_native_monotonic_nanos();
    observed = hull_quickjs_native_wait_observe(key);
    js_os_mutex_unlock(mutex);
    for (;;) {
        if (atomic_load_explicit(&cond->sequence, memory_order_acquire) != sequence)
            break;
        remaining = UINT64_MAX;
        if (timeout_ms != INT64_MAX) {
            elapsed = (hull_quickjs_native_monotonic_nanos() - start) / 1000000;
            if (elapsed >= (uint64_t)timeout_ms) {
                result = JS_OS_WAIT_TIMEOUT;
                break;
            }
            remaining = (uint64_t)timeout_ms - elapsed;
        }
        hull_quickjs_native_wait_after(key, observed, remaining);
        /* Observe first, then recheck our own sequence: unrelated/spurious
           wakes cannot turn Atomics.wait into a successful notification. */
        observed = hull_quickjs_native_wait_observe(key);
    }
    js_os_mutex_lock(mutex);
    return result;
}

static void js_os_cond_wait(JSOSCond *cond, JSOSMutex *mutex)
{
    js_os_cond_timedwait(cond, mutex, INT64_MAX);
}

static int64_t js_os_monotonic_ms(void)
{
    return hull_quickjs_native_monotonic_nanos() / 1000000;
}

#else
#include <errno.h>
#include <time.h>
#include <pthread.h>
#define JS_OS_WAIT_TIMEOUT ETIMEDOUT
typedef pthread_mutex_t JSOSMutex;
typedef pthread_cond_t JSOSCond;
#define JS_OS_MUTEX_INITIALIZER PTHREAD_MUTEX_INITIALIZER
#define js_os_mutex_lock pthread_mutex_lock
#define js_os_mutex_unlock pthread_mutex_unlock
#define js_os_cond_wait pthread_cond_wait
#define js_os_cond_destroy pthread_cond_destroy
#define js_os_cond_signal pthread_cond_signal

static void js_os_cond_init(JSOSCond *cond)
{
#if defined(__APPLE__)
    pthread_cond_init(cond, NULL);
#else
    pthread_condattr_t attr;
    pthread_condattr_init(&attr);
    pthread_condattr_setclock(&attr, CLOCK_MONOTONIC);
    pthread_cond_init(cond, &attr);
    pthread_condattr_destroy(&attr);
#endif
}

static int js_os_cond_timedwait(JSOSCond *cond, JSOSMutex *mutex,
                                int64_t timeout_ms)
{
    struct timespec ts;
#if defined(__APPLE__)
    ts.tv_sec = timeout_ms / 1000;
    ts.tv_nsec = (timeout_ms % 1000) * 1000000;
    return pthread_cond_timedwait_relative_np(cond, mutex, &ts);
#else
    clock_gettime(CLOCK_MONOTONIC, &ts);
    ts.tv_sec += timeout_ms / 1000;
    ts.tv_nsec += (timeout_ms % 1000) * 1000000;
    if (ts.tv_nsec >= 1000000000) {
        ts.tv_nsec -= 1000000000;
        ts.tv_sec++;
    }
    return pthread_cond_timedwait(cond, mutex, &ts);
#endif
}

static int64_t js_os_monotonic_ms(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}
#endif
#endif
