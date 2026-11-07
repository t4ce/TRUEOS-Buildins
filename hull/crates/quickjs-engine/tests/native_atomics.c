/* Actual QuickJS evaluations on independent host threads/runtimes sharing a
   buffer. pthread below belongs only to the host fixture/carrier simulator. */
#include "quickjs.h"
#include "quickjs-thread.h"
#include <assert.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef struct { JSRuntime *rt; JSContext *ctx; } Engine;
static _Alignas(8) _Atomic(int32_t) shared[16];
static _Atomic(unsigned) async_wakes;
static JSRuntime *async_owner;
extern uint64_t fixture_spurious_returns(void);

static uint64_t host_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}
static void pause_ms(unsigned ms) {
    struct timespec ts = { ms / 1000, (ms % 1000) * 1000000 };
    nanosleep(&ts, NULL);
}
static void no_free(JSRuntime *rt, void *opaque, void *ptr) {
    (void)rt; (void)opaque; (void)ptr;
}
static Engine engine(void) {
    Engine e = { JS_NewRuntime(), NULL };
    assert(e.rt);
    JS_SetMaxStackSize(e.rt, 0);
    JS_SetCanBlock(e.rt, 1);
    e.ctx = JS_NewContext(e.rt);
    assert(e.ctx);
    JSValue global = JS_GetGlobalObject(e.ctx);
    JSValue buffer = JS_NewArrayBuffer(e.ctx, (void *)shared, sizeof(shared), no_free, NULL, 1);
    assert(!JS_IsException(buffer));
    assert(JS_SetPropertyStr(e.ctx, global, "buffer", buffer) >= 0);
    JS_FreeValue(e.ctx, global);
    return e;
}
static void close_engine(Engine *e) { JS_FreeContext(e->ctx); JS_FreeRuntime(e->rt); }
static JSValue eval(Engine *e, const char *script) {
    JSValue v = JS_Eval(e->ctx, script, strlen(script), "native-atomics-test.js", JS_EVAL_TYPE_GLOBAL);
    if (JS_IsException(v)) {
        JSValue exception = JS_GetException(e->ctx);
        const char *text = JS_ToCString(e->ctx, exception);
        fprintf(stderr, "JS exception: %s\nscript: %s\n", text ? text : "?", script);
        abort();
    }
    return v;
}
static void equals(Engine *e, const char *script, const char *expected) {
    JSValue v = eval(e, script);
    const char *text = JS_ToCString(e->ctx, v);
    if (!text || strcmp(text, expected)) {
        fprintf(stderr, "expected %s, got %s: %s\n", expected, text ? text : "?", script);
        abort();
    }
    JS_FreeCString(e->ctx, text); JS_FreeValue(e->ctx, v);
}
static int number(Engine *e, const char *script) {
    JSValue v = eval(e, script); int32_t n;
    assert(JS_ToInt32(e->ctx, &n, v) == 0);
    JS_FreeValue(e->ctx, v); return n;
}
static void jobs(Engine *e) {
    JSContext *ctx;
    while (JS_IsJobPending(e->rt)) assert(JS_ExecutePendingJob(e->rt, &ctx) > 0);
}
static void reset(void) {
    for (unsigned i = 0; i < 16; i++) atomic_store(&shared[i], 0);
}
typedef struct { const char *script; char result[64]; } Worker;
static void *run_worker(void *opaque) {
    Worker *w = opaque; Engine e = engine();
    JSValue v = eval(&e, w->script);
    const char *text = JS_ToCString(e.ctx, v);
    assert(text); snprintf(w->result, sizeof(w->result), "%s", text);
    JS_FreeCString(e.ctx, text); JS_FreeValue(e.ctx, v);
    close_engine(&e); return NULL;
}
static void until_ready(int value) {
    uint64_t deadline = host_ms() + 5000;
    while (atomic_load(&shared[1]) != value) {
        assert(host_ms() < deadline); sched_yield();
    }
}
static void notify_one(Engine *e) {
    uint64_t deadline = host_ms() + 5000;
    while (!number(e, "Atomics.notify(new Int32Array(buffer),0,1)")) {
        assert(host_ms() < deadline); sched_yield();
    }
}
static void async_waker(JSRuntime *rt, void *opaque) {
    assert(rt == async_owner); assert(opaque == &async_wakes);
    atomic_fetch_add(&async_wakes, 1);
}
static JSOSMutex counter_mutex = JS_OS_MUTEX_INITIALIZER;
static unsigned counter;
static void *mutex_worker(void *unused) {
    (void)unused;
    for (int i = 0; i < 20000; i++) {
        js_os_mutex_lock(&counter_mutex); counter++; js_os_mutex_unlock(&counter_mutex);
    }
    return NULL;
}
static void *class_worker(void *opaque) {
    JSClassID *ids = opaque;
    for (int i = 0; i < 2000; i++) { ids[i] = 0; assert(JS_NewClassID(&ids[i]) == ids[i]); }
    return NULL;
}
static int compare_ids(const void *a, const void *b) {
    JSClassID x = *(const JSClassID *)a, y = *(const JSClassID *)b;
    return (x > y) - (x < y);
}

int main(void) {
    Engine e = engine(); reset();
    equals(&e, "Atomics.wait(new Int32Array(buffer),0,9,Infinity)", "not-equal");
    equals(&e, "Atomics.wait(new Int32Array(buffer),0,0,0)", "timed-out");
    equals(&e, "Atomics.wait(new Int32Array(buffer),0,0,-10)", "timed-out");
    equals(&e, "Atomics.wait(new BigInt64Array(buffer),0,1n,100)", "not-equal");
    equals(&e, "Atomics.wait(new BigInt64Array(buffer),0,0n,0)", "timed-out");
    uint64_t start = host_ms();
    equals(&e, "Atomics.wait(new Int32Array(buffer),0,0,25)", "timed-out");
    assert(host_ms() - start >= 24 && host_ms() - start < 2000);
    JS_SetCanBlock(e.rt, 0);
    equals(&e, "(()=>{try{Atomics.wait(new Int32Array(buffer),0,0,0)}catch(e){return e instanceof TypeError}})()", "true");
    JS_SetCanBlock(e.rt, 1);
    puts("PASS sync mismatch/zero/negative/finite/BigInt/can-block");

    Worker waiter = { "Atomics.store(new Int32Array(buffer),1,1);Atomics.wait(new Int32Array(buffer),0,0)", "" };
    pthread_t thread; pthread_create(&thread, NULL, run_worker, &waiter);
    until_ready(1); notify_one(&e); pthread_join(thread, NULL);
    assert(!strcmp(waiter.result, "ok"));
    puts("PASS cross-runtime infinite wait/notify");

    reset();
    Worker stress = { "(()=>{let a=new Int32Array(buffer);for(let i=1;i<=1000;i++){Atomics.store(a,1,i);if(Atomics.wait(a,0,0,5000)!=='ok')throw Error('lost wake')}return 'ok'})()", "" };
    pthread_create(&thread, NULL, run_worker, &stress);
    for (int i = 1; i <= 1000; i++) { until_ready(i); notify_one(&e); }
    pthread_join(thread, NULL); assert(!strcmp(stress.result, "ok"));
    puts("PASS 1000 cross-runtime registration/park/notify races");

    reset();
    equals(&e, "(()=>{let a=new Int32Array(buffer);return [Atomics.waitAsync(a,0,1).async,Atomics.waitAsync(a,0,1).value,Atomics.waitAsync(a,0,0,0).async,Atomics.waitAsync(a,0,0,-1).value].join(',')})()", "false,not-equal,false,timed-out");
    equals(&e, "globalThis.results=[];for(let i=0;i<3;i++)Atomics.waitAsync(new Int32Array(buffer),0,0).value.then(v=>results.push(v));Atomics.notify(new Int32Array(buffer),0,0)", "0");
    equals(&e, "Atomics.notify(new Int32Array(buffer),0,1)", "1"); jobs(&e);
    equals(&e, "results.join(',')", "ok");
    equals(&e, "Atomics.notify(new Int32Array(buffer),0,Infinity)", "2"); jobs(&e);
    equals(&e, "results.join(',')", "ok,ok,ok");
    assert(JS_AtomicsPollWaiters(e.ctx) == -1);
    puts("PASS waitAsync fast paths/count zero/one/all/Promise delivery");

    equals(&e, "globalThis.answer='pending';Atomics.waitAsync(new Int32Array(buffer),0,0,25).value.then(v=>answer=v);answer", "pending");
    int64_t delay = JS_AtomicsPollWaiters(e.ctx); assert(delay > 0 && delay <= 25);
    pause_ms(30); assert(JS_AtomicsPollWaiters(e.ctx) == 0); jobs(&e);
    equals(&e, "answer", "timed-out"); assert(JS_AtomicsPollWaiters(e.ctx) == -1);
    puts("PASS waitAsync monotonic deadline/poll/job resolution");

    async_owner = e.rt; atomic_store(&async_wakes, 0);
    JS_AtomicsSetAsyncWaker(async_waker, &async_wakes);
    equals(&e, "globalThis.answer='pending';Atomics.waitAsync(new BigInt64Array(buffer),0,0n,Infinity).value.then(v=>answer=v);answer", "pending");
    Worker notifier = { "Atomics.notify(new BigInt64Array(buffer),0,1)", "" };
    pthread_create(&thread, NULL, run_worker, &notifier); pthread_join(thread, NULL);
    assert(!strcmp(notifier.result, "1")); assert(atomic_load(&async_wakes) == 1);
    assert(JS_AtomicsPollWaiters(e.ctx) == 0); jobs(&e);
    equals(&e, "answer", "ok");
    JS_AtomicsSetAsyncWaker(NULL, NULL);
    puts("PASS cross-runtime BigInt waitAsync native owner waker");

    Engine cancelled = engine();
    equals(&cancelled, "Atomics.waitAsync(new Int32Array(buffer),0,0).async", "true");
    close_engine(&cancelled);
    equals(&e, "Atomics.notify(new Int32Array(buffer),0)", "0");
    puts("PASS runtime teardown unlinks pending waitAsync");

    pthread_t workers[4]; JSClassID ids[8000];
    for (int i = 0; i < 4; i++) pthread_create(&workers[i], NULL, mutex_worker, NULL);
    for (int i = 0; i < 4; i++) pthread_join(workers[i], NULL);
    assert(counter == 80000);
    for (int i = 0; i < 4; i++) pthread_create(&workers[i], NULL, class_worker, ids + 2000*i);
    for (int i = 0; i < 4; i++) pthread_join(workers[i], NULL);
    qsort(ids, 8000, sizeof(*ids), compare_ids);
    for (int i = 1; i < 8000; i++) assert(ids[i] > ids[i-1]);
    puts("PASS contended in-place mutex/8000 concurrent class IDs");
    close_engine(&e);
#ifdef HULL_QUICKJS_NATIVE_WAIT
    assert(sizeof(JSOSMutex) == 4 && sizeof(JSOSCond) == 4);
    assert(fixture_spurious_returns() > 100);
    printf("PASS native in-place size/spurious wake rejection (%llu injected)\n", (unsigned long long)fixture_spurious_returns());
#endif
    return 0;
}
