// SPDX-License-Identifier: MPL-2.0
// Private instrumentation. The drawable block retains its own bounded log,
// never a Rust pointer. No callback can outlive Rust-owned callback storage.
#import <AppKit/AppKit.h>
#import <Metal/Metal.h>
#import <QuartzCore/QuartzCore.h>
#include <stdint.h>
typedef struct { uint64_t id; double time; } CuePresented;
@interface CuePresentationLog : NSObject {
@public
    NSLock *lock;
    CuePresented entries[8192];
    uint64_t count, nextId, overflow;
}
@end
@implementation CuePresentationLog
- (instancetype)init { if ((self = [super init])) lock = [NSLock new]; return self; }
@end
@interface CueObservedLayer : CAMetalLayer
@property(strong) CuePresentationLog *log;
@end
@implementation CueObservedLayer
- (id<CAMetalDrawable>)nextDrawable {
    id<CAMetalDrawable> drawable = [super nextDrawable];
    if (!drawable) return nil;
    CuePresentationLog *log = self.log;
    [log->lock lock]; uint64_t number = ++log->nextId; [log->lock unlock];
    [drawable addPresentedHandler:^(id<MTLDrawable> shown) {
        [log->lock lock];
        if (log->count < 8192) log->entries[log->count++] = (CuePresented){number, shown.presentedTime};
        else ++log->overflow;
        [log->lock unlock];
    }];
    return drawable;
}
@end
void *cue_present_layer(void *viewPointer) {
    NSView *view = (__bridge NSView *)viewPointer;
    if (![NSThread isMainThread] || !view.window) return NULL;
    CueObservedLayer *layer = [CueObservedLayer layer];
    layer.log = [CuePresentationLog new];
    layer.frame = view.bounds;
    layer.contentsScale = view.window.backingScaleFactor;
    view.wantsLayer = YES;
    view.layer = layer;
    return (__bridge_retained void *)layer;
}
void cue_present_layer_release(void *layer) { (void)CFBridgingRelease(layer); }
double cue_present_clock(void) { return CACurrentMediaTime(); }
uint64_t cue_present_last_id(void *pointer) {
    CuePresentationLog *log = ((__bridge CueObservedLayer *)pointer).log;
    [log->lock lock]; uint64_t n = log->nextId; [log->lock unlock]; return n;
}
uint64_t cue_present_drain(void *pointer, CuePresented *out, uint64_t capacity, uint64_t *overflow) {
    CuePresentationLog *log = ((__bridge CueObservedLayer *)pointer).log;
    [log->lock lock];
    uint64_t n = MIN(log->count, capacity);
    memcpy(out, log->entries, n * sizeof(*out));
    memmove(log->entries, log->entries+n, (log->count-n)*sizeof(*out));
    log->count -= n; *overflow = log->overflow;
    [log->lock unlock]; return n;
}
uint64_t cue_present_refresh(void *pointer) {
    NSView *view = (__bridge NSView *)pointer;
    return view.window.screen.maximumFramesPerSecond;
}
