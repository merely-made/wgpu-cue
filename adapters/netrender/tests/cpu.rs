// SPDX-License-Identifier: MPL-2.0
#![cfg(feature = "cpu")]
use webcue::*;
use wgpu_cue_netrender::{Scene, cpu::*};

fn viewport() -> Viewport {
    Viewport {
        width: 32,
        height: 32,
        scale: 1.0,
    }
}
fn scope() -> Scope {
    Scope {
        owner: 91,
        generation: 1,
    }
}
fn snapshot() -> SceneSnapshot {
    let mut scene = Scene::new(32, 32);
    scene.push_rect(0., 0., 16., 16., [0.5, 0., 0., 0.5]);
    SceneSnapshot::capture(&scene, RenderLimits::default()).unwrap()
}
fn source() -> CpuSceneSource {
    CpuSceneSource::new(scope(), viewport(), RenderLimits::default()).unwrap()
}

#[test]
fn superseded_job_cannot_replace_current_pixels() {
    let mut source = source();
    let old = source.job(1, snapshot()).unwrap();
    let new = source.job(2, snapshot()).unwrap();
    let old = rasterize(&old, RenderLimits::default()).unwrap();
    assert!(matches!(
        source.complete(old),
        Err(RenderError::Control(Error::Stale))
    ));
    assert!(source.try_acquire_frame().unwrap().is_none());
    let frame = rasterize(&new, RenderLimits::default()).unwrap();
    source.complete(frame.clone()).unwrap();
    assert!(matches!(
        source.complete(frame),
        Err(RenderError::Control(Error::Stale))
    ));
    assert!(
        matches!(source.try_acquire_frame().unwrap(), Some(Frame::Cpu(frame)) if frame.stamp == new.stamp)
    );
    assert!(source.try_acquire_frame().unwrap().is_none());
}

#[test]
fn resize_hide_and_retirement_invalidate_pending_jobs() {
    let mut source = source();
    let job = source.job(1, snapshot()).unwrap();
    source.set_visible(false).unwrap();
    source.set_visible(true).unwrap();
    assert!(matches!(
        source.complete(rasterize(&job, RenderLimits::default()).unwrap()),
        Err(RenderError::Control(Error::Stale))
    ));
    let job = source.job(2, snapshot()).unwrap();
    source
        .set_viewport(Viewport {
            width: 64,
            ..viewport()
        })
        .unwrap();
    source.set_viewport(viewport()).unwrap();
    assert!(matches!(
        source.complete(rasterize(&job, RenderLimits::default()).unwrap()),
        Err(RenderError::Control(Error::Stale))
    ));
    source.retire().unwrap();
    assert!(matches!(
        source.job(3, snapshot()),
        Err(RenderError::Control(Error::Retired))
    ));
    assert!(matches!(source.try_acquire_frame(), Err(Error::Retired)));
}

#[test]
fn cpu_output_is_owned_premultiplied_rgba_with_transparent_clear() {
    let job = source().job(1, snapshot()).unwrap();
    let frame = rasterize(&job, RenderLimits::default()).unwrap();
    frame.validate().unwrap();
    assert_eq!(frame.alpha, Alpha::Premultiplied);
    assert_eq!(frame.encoding, Encoding::Srgb);
    let red = &frame.rgba[(8 * 32 + 8) * 4..][..4];
    assert!(
        red[0].abs_diff(128) < 3 && red[1] == 0 && red[2] == 0 && red[3].abs_diff(128) < 3,
        "{red:?}"
    );
    assert_eq!(&frame.rgba[(24 * 32 + 24) * 4..][..4], [0; 4]);
}

#[test]
fn fragment_handles_and_source_budget_are_refused_before_raster() {
    let mut scene = Scene::new(32, 32);
    scene.place_fragment(7, netrender::Transform::IDENTITY);
    assert!(matches!(
        SceneSnapshot::capture(&scene, RenderLimits::default()),
        Err(RenderError::UnresolvedResource("registry-local fragment"))
    ));
    assert!(matches!(
        SceneSnapshot::from_bytes(
            vec![0; 10],
            RenderLimits {
                snapshot_bytes: 9,
                ..Default::default()
            }
        ),
        Err(RenderError::Budget("snapshot bytes"))
    ));
    assert!(matches!(
        SceneSnapshot::capture(
            &Scene::new(32, 32),
            RenderLimits {
                frame_bytes: 4,
                ..Default::default()
            }
        ),
        Err(RenderError::Budget("frame pixels"))
    ));
}

#[test]
fn snapshot_decode_mints_receiver_local_blob_identities() {
    let mut scene = Scene::new(32, 32);
    scene.fonts[0].data = netrender::peniko::Blob::new(std::sync::Arc::new(Vec::<u8>::new()));
    scene
        .image_sources
        .insert(1, netrender::ImageData::from_bytes(1, 1, vec![255; 4]));
    let snapshot = SceneSnapshot::capture(&scene, RenderLimits::default()).unwrap();
    let a = snapshot.decode(RenderLimits::default()).unwrap();
    let b = snapshot.decode(RenderLimits::default()).unwrap();
    assert_eq!(a.fonts[0].data.id(), Scene::new(1, 1).fonts[0].data.id());
    assert_ne!(
        a.image_sources[&1].data.id(),
        scene.image_sources[&1].data.id()
    );
    assert_ne!(a.image_sources[&1].data.id(), b.image_sources[&1].data.id());
    assert_eq!(
        a.image_sources[&1].data.as_ref(),
        b.image_sources[&1].data.as_ref()
    );
}

#[test]
fn foreign_identity_and_invalid_pixels_preserve_the_current_job() {
    let mut source = source();
    let job = source.job(1, snapshot()).unwrap();
    let frame = rasterize(&job, RenderLimits::default()).unwrap();
    let mut foreign = frame.clone();
    foreign.stamp.scope.generation += 1;
    assert!(matches!(
        source.complete(foreign),
        Err(RenderError::Control(Error::Stale))
    ));
    let mut invalid = frame.clone();
    invalid.alpha = Alpha::Straight;
    assert!(matches!(
        source.complete(invalid),
        Err(RenderError::Control(Error::Invalid(_)))
    ));
    source.complete(frame).unwrap();
    assert!(matches!(
        source.try_acquire_frame().unwrap(),
        Some(Frame::Cpu(_))
    ));
}

#[test]
fn image_pixels_are_owned_and_unresolved_image_keys_are_refused() {
    let mut scene = Scene::new(32, 32);
    scene.push_image(
        0.,
        0.,
        16.,
        16.,
        7,
        netrender::ImageData::from_bytes(1, 1, vec![0, 0, 255, 128]),
    );
    let snapshot = SceneSnapshot::capture(&scene, RenderLimits::default()).unwrap();
    let job = source().job(1, snapshot).unwrap();
    let frame = rasterize(&job, RenderLimits::default()).unwrap();
    let pixel = &frame.rgba[(8 * 32 + 8) * 4..][..4];
    assert!(
        pixel[0] == 0 && pixel[1] == 0 && pixel[2].abs_diff(128) < 3 && pixel[3].abs_diff(128) < 3,
        "{pixel:?}"
    );
    scene.image_sources.clear();
    assert!(matches!(
        SceneSnapshot::capture(&scene, RenderLimits::default()),
        Err(RenderError::UnresolvedResource(
            "image key without owned pixels"
        ))
    ));
}

#[test]
fn receiver_enforces_its_resource_budget_before_handing_off_a_job() {
    let mut scene = Scene::new(32, 32);
    scene
        .image_sources
        .insert(1, netrender::ImageData::from_bytes(1, 1, vec![255; 4]));
    let snapshot = SceneSnapshot::capture(&scene, RenderLimits::default()).unwrap();
    let mut source = CpuSceneSource::new(
        scope(),
        viewport(),
        RenderLimits {
            resource_bytes: 3,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(
        source.job(1, snapshot),
        Err(RenderError::Budget("font/image source bytes"))
    ));
    // Rejected admission did not consume the revision.
    let empty = SceneSnapshot::capture(&Scene::new(32, 32), RenderLimits::default()).unwrap();
    assert!(source.job(1, empty).is_ok());
}
