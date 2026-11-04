// SPDX-License-Identifier: MPL-2.0
use super::*;
use grafting::dx12_surface::SharedDx12Surface;
struct Pending {
    lease: Lease,
    job: Option<RenderJob>,
    rendered: Option<TextureFrame>,
    copying: bool,
    published: bool,
    producer_done: bool,
    handles: Option<(OwnedHandle, OwnedHandle)>,
}
pub fn helper() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let init: Init =
        read_frame_with_limit(&mut io::stdin().lock(), LIMIT)?.ok_or("missing DX12 init")?;
    let execution = ExecutionId::from(init.identity);
    let mut resources = ResourceReceiver::new(execution, RenderLimits::default())?;
    if init.schema != SCHEMA || execution.incarnation == 0 {
        return Err("invalid DX12 init".into());
    }
    let (adapter, host, native) = gpu_for(Some(init.luid))?;
    let mut viewport = Viewport {
        width: init.width,
        height: init.height,
        scale: 1.0,
    };
    let mut source = HybridSceneSource::new(
        execution.scope,
        &host.context(viewport)?,
        RenderLimits::default(),
    )?;
    let deadline = Instant::now() + TIMEOUT;
    while !source.poll_idle()? {
        if Instant::now() > deadline {
            return Err("Hybrid startup deadline".into());
        }
        thread::sleep(Duration::from_millis(1));
    }
    let size = FrameSize::new(init.width, init.height);
    let mut surfaces = [
        SharedDx12Surface::new(&native, size)?,
        SharedDx12Surface::new(&native, size)?,
    ];
    let mut output = io::stdout().lock();
    let mut send = |message| -> io::Result<()> {
        write_frame_with_limit(
            &mut output,
            &encode_with_limit(&Packet::new(execution, init.nonce, message), LIMIT)?,
            LIMIT,
        )
    };
    send(Message::Hello {
        adapter: adapter.get_info().name,
        luid: grafting::dx12_surface::adapter_luid(&native)?,
    })?;
    let (tx, jobs) = mpsc::sync_channel::<io::Result<Input>>(4);
    thread::Builder::new()
        .name("cue-dx12-input".into())
        .spawn(move || {
            let mut input = io::stdin().lock();
            loop {
                match read_frame_with_limit::<Input>(&mut input, LIMIT) {
                    Ok(Some(value)) => {
                        if tx.send(Ok(value)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        break;
                    }
                }
            }
        })?;
    let mut pool = FrameSlotPool::new(execution.scope, 2)?;
    let mut pending: BTreeMap<u64, Pending> = BTreeMap::new();
    let mut waiting = VecDeque::new();
    let mut active = None;
    let mut revision = 0;
    let mut allocation = 0;
    let mut last_activity = Instant::now();
    loop {
        native.device.poll(wgpu::PollType::Poll)?;
        if let Some(epoch) = active
            && let Rendered::Changed(frame) = source.render(&host.context(viewport)?, true)?
        {
            pending
                .get_mut(&epoch)
                .ok_or("missing DX12 render")?
                .rendered = Some(frame);
            active = None;
        }
        for state in pending.values_mut() {
            let surface = &mut surfaces[state.lease.slot as usize];
            if !state.copying
                && let Some(frame) = state.rendered.as_ref()
            {
                // SAFETY: Hybrid renders on this exact host device/queue.
                unsafe {
                    surface.copy_from(&frame.texture)?;
                }
                state.copying = true;
            }
            if state.copying && !state.producer_done && surface.poll_ready()? {
                pool.producer_done(state.lease.slot(execution.scope))?;
                state.producer_done = true;
                state.rendered = None;
            }
            // A shared fence is poisoned with UINT64_MAX when its producer
            // device exits. Publish only after observing genuine producer
            // completion, so retained frames remain valid across peer death.
            if state.producer_done && !state.published {
                let (resource, fence, value) = surface.export()?;
                let size = surface.size();
                send(Message::Ready {
                    lease: state.lease,
                    width: size.width,
                    height: size.height,
                    resource: resource.as_raw_handle() as usize as u64,
                    fence: fence.as_raw_handle() as usize as u64,
                    value,
                })?;
                state.handles = Some((resource, fence));
                state.published = true;
            }
        }
        if active.is_none()
            && let Some((&epoch, state)) = pending.iter_mut().find(|(_, state)| state.job.is_some())
        {
            let job = state.job.take().ok_or("missing DX12 job")?;
            let size = job.snapshot.size();
            viewport = Viewport {
                width: size.width,
                height: size.height,
                scale: 1.0,
            };
            source.set_viewport(viewport)?;
            source.submit(
                &host.context(viewport)?,
                job.stamp.content_revision,
                &job.snapshot,
            )?;
            active = Some(epoch);
        }
        match jobs.try_recv() {
            Ok(input) => {
                last_activity = Instant::now();
                match input? {
                    Input::Job(wire) if waiting.len() < 2 => {
                        waiting.push_back(wire.request_with(&mut resources)?);
                    }
                    Input::Job(_) => return Err("DX12 helper job bound exceeded".into()),
                    Input::Release(lease) => {
                        if pending
                            .get(&lease.epoch)
                            .is_some_and(|s| s.lease == lease && s.published)
                        {
                            // SAFETY: exact current lease from the trusted host's
                            // private input pipe; host waits for final alias release.
                            unsafe {
                                surfaces[lease.slot as usize].acknowledge_release()?;
                            }
                            pool.consumer_released(lease.slot(execution.scope))?;
                            pending.remove(&lease.epoch);
                            send(Message::Released(lease))?;
                        } else {
                            send(Message::Stale(lease))?;
                        }
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                validation::check(&native)?;
                return Ok(());
            }
        }
        if pool.occupied() < 2
            && let Some(request) = waiting.pop_front()
        {
            if request.execution != execution
                || request.job.stamp.scope != execution.scope
                || request.job.stamp.content_revision <= revision
                || request.job.stamp.allocation_epoch <= allocation
            {
                return Err("stale DX12 job".into());
            }
            revision = request.job.stamp.content_revision;
            allocation = request.job.stamp.allocation_epoch;
            let slot = pool.acquire()?;
            let size = request.job.snapshot.size();
            if surfaces[slot.slot as usize].size() != size {
                surfaces[slot.slot as usize] = SharedDx12Surface::new(&native, size)?;
            }
            let lease = Lease {
                slot: slot.slot,
                epoch: slot.epoch,
                revision,
                allocation: request.job.stamp.allocation_epoch,
            };
            pending.insert(
                lease.epoch,
                Pending {
                    lease,
                    job: Some(request.job),
                    rendered: None,
                    copying: false,
                    published: false,
                    producer_done: false,
                    handles: None,
                },
            );
        }
        validation::check(&native)?;
        if Instant::now().duration_since(last_activity) > TIMEOUT {
            return Err("DX12 helper idle deadline".into());
        }
        thread::sleep(Duration::from_millis(1));
    }
}
