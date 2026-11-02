// SPDX-License-Identifier: MPL-2.0
// Private bounded proof transport. No numeric port names cross this boundary.
#include <mach/mach.h>
#include <servers/bootstrap.h>
#include <bsm/libbsm.h>
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#define CAP 4096
struct packet {
    mach_msg_header_t header;
    mach_msg_body_t body;
    mach_msg_port_descriptor_t port;
    uint32_t length;
    uint8_t bytes[CAP];
    uint8_t trailer[MAX_TRAILER_SIZE];
};
int cue_receive_port(uint32_t *out) {
    kern_return_t r = mach_port_allocate(mach_task_self(), MACH_PORT_RIGHT_RECEIVE, out);
    if (r != KERN_SUCCESS) return r;
    r = mach_port_insert_right(mach_task_self(), *out, *out, MACH_MSG_TYPE_MAKE_SEND);
    if (r != KERN_SUCCESS) mach_port_destroy(mach_task_self(), *out);
    return r;
}
int cue_register(const char *name, uint32_t port) { return bootstrap_register(bootstrap_port, (char *)name, port); }
int cue_lookup(const char *name, uint32_t *out) { return bootstrap_look_up(bootstrap_port, (char *)name, out); }
void cue_destroy_port(uint32_t port) { mach_port_destroy(mach_task_self(), port); }
int cue_send(uint32_t remote, const uint8_t *bytes, uint32_t length, uint32_t right) {
    if (length > CAP) return KERN_INVALID_ARGUMENT;
    struct packet p = {0};
    p.header.msgh_bits = MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, 0) | MACH_MSGH_BITS_COMPLEX;
    p.header.msgh_remote_port = remote;
    p.header.msgh_size = (mach_msg_size_t)((offsetof(struct packet, bytes) + length + 3) & ~3);
    p.body.msgh_descriptor_count = 1;
    p.port.name = right;
    p.port.disposition = MACH_MSG_TYPE_COPY_SEND;
    p.port.type = MACH_MSG_PORT_DESCRIPTOR;
    p.length = length;
    memcpy(p.bytes, bytes, length);
    // Bounded send: a stalled peer cannot block the engine thread indefinitely.
    return mach_msg(&p.header, MACH_SEND_MSG | MACH_SEND_TIMEOUT, p.header.msgh_size, 0, MACH_PORT_NULL, 100, MACH_PORT_NULL);
}
int cue_receive(uint32_t local, uint8_t *bytes, uint32_t *length, uint32_t *right, int32_t *pid) {
    struct packet p = {0};
    kern_return_t r = mach_msg(&p.header, MACH_RCV_MSG | MACH_RCV_TIMEOUT |
        MACH_RCV_TRAILER_TYPE(MACH_MSG_TRAILER_FORMAT_0) |
        MACH_RCV_TRAILER_ELEMENTS(MACH_RCV_TRAILER_AUDIT), 0, sizeof(p), local, 0, MACH_PORT_NULL);
    if (r == MACH_RCV_TIMED_OUT) return 1;
    if (r != KERN_SUCCESS) return r;
    if (!(p.header.msgh_bits & MACH_MSGH_BITS_COMPLEX) || p.header.msgh_size < offsetof(struct packet, bytes) ||
        p.body.msgh_descriptor_count != 1 || p.port.type != MACH_MSG_PORT_DESCRIPTOR ||
        (p.port.name != MACH_PORT_NULL && p.port.disposition != MACH_MSG_TYPE_PORT_SEND) ||
        p.length > CAP || p.header.msgh_size != ((offsetof(struct packet, bytes) + p.length + 3) & ~3)) {
        mach_msg_destroy(&p.header); return KERN_INVALID_ARGUMENT;
    }
    mach_msg_audit_trailer_t *t = (void *)((uint8_t *)&p + ((p.header.msgh_size + 3) & ~3));
    if (t->msgh_trailer_type != MACH_MSG_TRAILER_FORMAT_0 || t->msgh_trailer_size < sizeof(*t)) {
        mach_msg_destroy(&p.header); return KERN_INVALID_ARGUMENT;
    }
    *pid = audit_token_to_pid(t->msgh_audit);
    *length = p.length;
    memcpy(bytes, p.bytes, p.length);
    *right = p.port.name; // transfer exactly one received send right to Rust
    return 0;
}
