use std::{
    any::TypeId, cell::RefCell, marker::PhantomData, rc::Rc, sync::RwLock, thread::ThreadId,
};

use bevy::{
    ecs::system::{Command, SystemBuffer},
    prelude::*,
    utils::{HashMap, HashSet},
};
use polako_constructivism::*;

pub struct FlowPlugin;
impl Plugin for FlowPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, flow_loop);
        app.init_resource::<FlowIteration>();
        app.insert_resource(FlowResource::new());
        app.insert_resource(BindTargets::new());
    }
}

pub fn flow_loop(world: &mut World) {
    let flow = world.resource::<FlowResource>().clone();

    // start the loop
    world.resource_mut::<FlowIteration>().reset();
    loop {
        let mut schedule_ref = flow.schedule.borrow_mut();
        // apply deferred scheduler edits
        flow.queue
            .take()
            .into_iter()
            .for_each(|c| c(&mut schedule_ref));

        // apply deferred world edits
        flow.commands.take().into_iter().for_each(|c| c(world));

        // process schedule
        schedule_ref.run(world);

        // any changes?
        if world.resource_mut::<FlowIteration>().repeats() {
            // start the new iteration
            world.resource_mut::<FlowIteration>().step();
            continue;
        } else {
            break;
        }
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, SystemSet)]
pub enum FlowSet {
    CleanupChanges,
    CleanupReaders,
    CollectChanges,
    Read,
    CleanupWriteChanges,
    Write,
    PopulateChanges,
}

fn first_iteration(iteration: Res<FlowIteration>) -> bool {
    iteration.first()
}

fn cleanup_changes<T: Component, V: Bindable>(changes: Changes<T, V>) {
    changes.clear();
}

fn collect_changes<T: Component>(
    changed: Query<Entity, Changed<T>>,
    mut changes: ResMut<ChangedEntities<T>>,
) {
    changes.entities.extend(changed.iter());
}

fn cleanup_component_readers<S: Component, T: Component, V: Bindable>(
    mut sources: Query<&mut ComponentBindSources<S, T, V>>,
    mut targets: ResMut<BindTargets>,
    mut removals: RemovedComponents<BindTarget>,
) {
    for target in removals.iter() {
        for source in targets.0.remove(&target).unwrap_or_default().iter() {
            if let Ok(mut source) = sources.get_mut(*source) {
                source.0.remove(&target);
            }
        }
    }
}

fn read_component_changes<S: Component, T: Component, V: Bindable>(
    components: Query<(&ComponentBindSources<S, T, V>, &S)>,
    changed: Res<ChangedEntities<S>>,
    changes: Changes<T, V>,
) {
    for (sources, component) in components.iter_many(changed.entities.iter()) {
        for items in sources.0.values() {
            for source in items.iter() {
                let value = source.read.read(&component).get();
                changes.send(ApplyChange {
                    value,
                    target: source.target,
                    writer: source.writer.clone(),
                })
            }
        }
    }
}

fn cleanup_resource_readers<S: Resource, T: Component, V: Bindable>(
    mut sources: ResMut<ResourceBindSources<S, T, V>>,
    mut removals: RemovedComponents<BindTarget>,
) {
    for target in removals.iter() {
        sources.0.remove(&target);
    }
}

fn read_resource_changes<S: Resource, T: Component, V: Bindable>(
    res: Res<S>,
    sources: Res<ResourceBindSources<S, T, V>>,
    changes: Changes<T, V>,
) {
    if res.is_changed() {
        for sources in sources.0.values() {
            for source in sources.iter() {
                let value = source.read.read(&res).get();
                changes.send(ApplyChange {
                    value,
                    target: source.target,
                    writer: source.writer.clone(),
                })
            }
        }
    }
}

fn cleanup_write_component_changes<T: Component>(changed_entities: Res<Channel<ChangedEntity<T>>>) {
    changed_entities.clear()
}

fn write_component_changes<T: Component, V: Bindable>(
    mut components: Query<(Entity, &mut T)>,
    changes: Changes<T, V>,
    changed_entities: Res<Channel<ChangedEntity<T>>>,
) {
    changes.recv(|change| {
        let Ok((entity, mut component)) = components.get_mut(change.target) else {
            return;
        };
        if change.writer.read(component.as_ref()).as_ref() == &change.value {
            return;
        }
        change
            .writer
            .write(component.as_mut(), change.value.clone());
        changed_entities.send(ChangedEntity::new(entity));
    });
}

fn populate_changes<T: Component>(
    changes: Res<Channel<ChangedEntity<T>>>,
    mut changed_entities: ResMut<ChangedEntities<T>>,
    mut populated: Local<HashSet<Entity>>,
    mut flow: Deferred<FlowLoopControl>,
) {
    populated.clear();
    changed_entities.entities.clear();
    changes.recv(|change| {
        flow.repeat();
        if populated.insert(change.entity) {
            changed_entities.entities.push(change.entity)
        }
    })
}

#[derive(Resource, Clone, Copy, Default)]
enum FlowIteration {
    #[default]
    First,
    Repeat,
    Break,
}

impl FlowIteration {
    /// Returns true if is is first iteration in the flow_loop
    fn first(&self) -> bool {
        matches!(self, FlowIteration::First)
    }

    /// Returns true it we need one more iteration in the flow_loop
    fn repeats(&self) -> bool {
        matches!(self, FlowIteration::Repeat)
    }
    /// Resets the to the start of the flow_loop
    fn reset(&mut self) {
        *self = FlowIteration::First
    }
    /// Requests one more iteration in the flow_loop
    fn repeat(&mut self) {
        *self = FlowIteration::Repeat
    }
    /// Resets to the start of the non-first iteration of the flow_loop
    fn step(&mut self) {
        *self = FlowIteration::Break
    }
}

#[derive(Default)]
struct FlowLoopControl(bool);
impl FlowLoopControl {
    fn repeat(&mut self) {
        self.0 = true;
    }
}
impl SystemBuffer for FlowLoopControl {
    fn apply(&mut self, _: &bevy::ecs::system::SystemMeta, world: &mut World) {
        if self.0 {
            self.0 = false;
            world.resource_mut::<FlowIteration>().repeat();
        }
    }
}

#[derive(Resource, Deref, Clone)]
struct FlowResource(Rc<Flow>);
unsafe impl Send for FlowResource {}
unsafe impl Sync for FlowResource {}
impl FlowResource {
    fn new() -> Self {
        FlowResource(Rc::new(Flow::new()))
    }
}

struct Flow {
    schedule: RefCell<Schedule>,
    queue: RefCell<Vec<Box<dyn FnOnce(&mut Schedule)>>>,
    commands: RefCell<Vec<Box<dyn FnOnce(&mut World)>>>,
    registry: RegisteredSystems,
}

struct HashCell(RefCell<HashSet<TypeId>>);
impl HashCell {
    fn register<T: 'static, F: FnOnce()>(&self, func: F) {
        let id = TypeId::of::<T>();
        if self.0.borrow_mut().insert(id) {
            func()
        }
    }
}
struct RegisteredSystems {
    cleanup_changes: HashCell,
    read_component: HashCell,
    read_resource: HashCell,
    write: HashCell,
    populate_changes: HashCell,
}

impl RegisteredSystems {
    fn new() -> Self {
        RegisteredSystems {
            cleanup_changes: HashCell(RefCell::new(HashSet::new())),
            read_component: HashCell(RefCell::new(HashSet::new())),
            read_resource: HashCell(RefCell::new(HashSet::new())),
            write: HashCell(RefCell::new(HashSet::new())),
            populate_changes: HashCell(RefCell::new(HashSet::new())),
        }
    }
}

impl Flow {
    fn new() -> Self {
        let mut schedule = Schedule::default();
        schedule.configure_sets((
            FlowSet::CleanupReaders.after(FlowSet::CleanupChanges),
            FlowSet::CollectChanges.after(FlowSet::CleanupReaders),
            FlowSet::Read.after(FlowSet::CollectChanges),
            FlowSet::CleanupWriteChanges.after(FlowSet::Read),
            FlowSet::Write.after(FlowSet::CleanupWriteChanges),
            FlowSet::PopulateChanges.after(FlowSet::Write),
        ));
        // schedule.
        Self {
            schedule: RefCell::new(schedule),
            queue: RefCell::new(vec![]),
            commands: RefCell::new(vec![]),
            registry: RegisteredSystems::new(),
        }
    }

    fn edit_schedule<F: FnOnce(&mut Schedule) + 'static>(&self, func: F) {
        self.queue.borrow_mut().push(Box::new(func))
    }
    fn edit_world<F: FnOnce(&mut World) + 'static>(&self, func: F) {
        self.commands.borrow_mut().push(Box::new(func))
    }

    fn register_populate_systems<C: Component>(&self) {
        self.registry.populate_changes.register::<C, _>(|| {
            self.edit_schedule(|schedule| {
                schedule.add_systems(
                    collect_changes::<C>
                        .in_set(FlowSet::CollectChanges)
                        .run_if(first_iteration),
                );
                schedule.add_systems(populate_changes::<C>.in_set(FlowSet::PopulateChanges));
                schedule.add_systems(
                    cleanup_write_component_changes::<C>.in_set(FlowSet::CleanupWriteChanges),
                );
            });
            self.edit_world(|world| {
                world.insert_resource(Channel::<ChangedEntity<C>>::new());
                world.insert_resource(ChangedEntities::<C>::new());
            });
        });
    }

    fn register_component_read_systems<S: Component, T: Component, V: Bindable>(&self) {
        self.register_populate_systems::<S>();
        self.registry.cleanup_changes.register::<(T, V), _>(|| {
            self.edit_schedule(|schedule| {
                schedule.add_systems(cleanup_changes::<T, V>.in_set(FlowSet::CleanupChanges));
            });
        });
        self.registry.read_component.register::<(S, T, V), _>(|| {
            self.edit_schedule(|schedule| {
                schedule.add_systems(
                    cleanup_component_readers::<S, T, V>.in_set(FlowSet::CleanupReaders),
                );
                schedule.add_systems(read_component_changes::<S, T, V>.in_set(FlowSet::Read));
            });
        })
    }

    fn register_resource_read_systems<S: Resource, T: Component, V: Bindable>(&self) {
        self.registry.cleanup_changes.register::<(T, V), _>(|| {
            self.edit_schedule(|schedule| {
                schedule.add_systems(cleanup_changes::<T, V>.in_set(FlowSet::CleanupChanges));
            });
        });
        self.registry.read_resource.register::<(S, T, V), _>(|| {
            self.edit_schedule(|schedule| {
                schedule.add_systems(
                    cleanup_resource_readers::<S, T, V>.in_set(FlowSet::CleanupReaders),
                );
                schedule.add_systems(read_resource_changes::<S, T, V>.in_set(FlowSet::Read));
            });
        });
    }

    fn register_component_write_systems<T: Component, V: Bindable>(&self) {
        self.register_populate_systems::<T>();
        self.registry.write.register::<(T, V), _>(|| {
            self.edit_schedule(|schedule| {
                schedule.add_systems(write_component_changes::<T, V>.in_set(FlowSet::Write));
            });
            self.edit_world(|world| {
                world.insert_resource(Channel::<ApplyChange<T, V>>::new());
            });
        });
    }
}

pub trait WorldFlow {
    fn bind_component_to_component<S: Component, T: Component, V: Bindable>(
        &mut self,
        from: ComponentReader<S, V>,
        to: ComponentWriter<T, V>,
    );

    fn bind_resource_to_component<R: Resource, T: Component, V: Bindable>(
        &mut self,
        from: Reader<R, V>,
        to: ComponentWriter<T, V>,
    );
}
impl WorldFlow for World {
    fn bind_component_to_component<S: Component, T: Component, V: Bindable>(
        &mut self,
        from: ComponentReader<S, V>,
        to: ComponentWriter<T, V>,
    ) {
        // setup source

        // this component will be added to `from.entity`, all required generic systems will be
        // added to the `Flow` if needed
        let bind_source = BindSource {
            target: to.entity,
            read: from.reader,
            writer: to.writer,
        };
        {
            let mut e = self.entity_mut(from.entity);
            if !e.contains::<ComponentBindSources<S, T, V>>() {
                e.insert(ComponentBindSources::<S, T, V>(HashMap::new()));
            }
            e.get_mut::<ComponentBindSources<S, T, V>>()
                .unwrap()
                .0
                .entry(to.entity)
                .or_default()
                .push(bind_source);
        }

        // setup target
        self.entity_mut(to.entity).insert(BindTarget);
        self.resource_mut::<BindTargets>()
            .0
            .entry(to.entity)
            .or_default()
            .insert(from.entity);

        let flow = self.resource::<FlowResource>().clone();
        flow.register_component_read_systems::<S, T, V>();
        flow.register_component_write_systems::<T, V>();
    }

    fn bind_resource_to_component<S: Resource, T: Component, V: Bindable>(
        &mut self,
        from: Reader<S, V>,
        to: ComponentWriter<T, V>,
    ) {
        let bind_source = BindSource {
            target: to.entity,
            read: from,
            writer: to.writer,
        };

        self.entity_mut(to.entity).insert(BindTarget);
        self.get_resource_or_insert_with(ResourceBindSources::<S, T, V>::new)
            .0
            .entry(to.entity)
            .or_default()
            .push(bind_source);

        let flow = self.resource::<FlowResource>().clone();
        flow.register_resource_read_systems::<S, T, V>();
        flow.register_component_write_systems::<T, V>();
    }
}

pub trait Bindable: Send + Sync + Clone + std::fmt::Debug + PartialEq + 'static {}
impl<T: Send + Sync + Clone + PartialEq + std::fmt::Debug + 'static> Bindable for T {}

type Changes<'w, T, V> = Res<'w, Channel<ApplyChange<T, V>>>;
#[derive(Resource)]
struct Channel<T>(RwLock<HashMap<ThreadId, Rc<RefCell<Vec<T>>>>>);
unsafe impl<T> Send for Channel<T> {}
unsafe impl<T> Sync for Channel<T> {}
impl<T> Channel<T> {
    fn new() -> Self {
        Self(RwLock::new(HashMap::new()))
    }
    fn clear(&self) {
        for cell in self.0.read().unwrap().values() {
            cell.borrow_mut().clear()
        }
    }
    fn send(&self, event: T) {
        let id = std::thread::current().id();
        {
            let read = self.0.read().unwrap();
            let item = read.get(&id);
            if let Some(events) = item {
                events.borrow_mut().push(event);
                return;
            }
        }
        {
            self.0
                .write()
                .unwrap()
                .insert(id, Rc::new(RefCell::new(vec![event])));
        }
    }
    fn recv<F: FnMut(&T)>(&self, mut recv: F) {
        for cell in self.0.read().unwrap().values() {
            let borrow = cell.borrow();
            for item in borrow.iter() {
                recv(item)
            }
        }
    }
}

#[derive(Resource)]
struct ChangedEntities<C: Component> {
    entities: Vec<Entity>,
    marker: PhantomData<C>,
}
impl<C: Component> ChangedEntities<C> {
    fn new() -> Self {
        Self {
            entities: vec![],
            marker: PhantomData,
        }
    }
}

struct ChangedEntity<T> {
    entity: Entity,
    marker: PhantomData<T>,
}
impl<T> ChangedEntity<T> {
    fn new(entity: Entity) -> Self {
        Self {
            entity,
            marker: PhantomData,
        }
    }
}

struct BindSource<S, T, V: Bindable> {
    target: Entity,
    read: Reader<S, V>,
    writer: Writer<T, V>,
}
#[derive(Component)]
struct ComponentBindSources<S: Component, T: Component, V: Bindable>(
    HashMap<Entity, Vec<BindSource<S, T, V>>>,
);

#[derive(Resource)]
struct ResourceBindSources<S: Resource, T: Component, V: Bindable>(
    HashMap<Entity, Vec<BindSource<S, T, V>>>,
);
impl<S: Resource, T: Component, V: Bindable> ResourceBindSources<S, T, V> {
    fn new() -> Self {
        Self(HashMap::new())
    }
}

#[derive(Component)]
struct BindTarget;

#[derive(Resource)]
struct BindTargets(HashMap<Entity, HashSet<Entity>>);
impl BindTargets {
    fn new() -> Self {
        BindTargets(HashMap::new())
    }
}

// pub struct HandleChange<W: W V: Bindable> {
//     handler:
// }

#[derive(Event)]
struct ApplyChange<H: Component, V: Bindable> {
    target: Entity,
    writer: Writer<H, V>,
    value: V,
}

pub trait EntityProp<H: Component, V: Bindable> {
    fn get(&self, value: impl Into<Reader<H, V>>) -> ComponentReader<H, V>;
    fn set(&self, value: impl Into<Writer<H, V>>) -> ComponentWriter<H, V>;
}

impl<H: Component, V: Bindable> EntityProp<H, V> for Entity {
    fn get(&self, value: impl Into<Reader<H, V>>) -> ComponentReader<H, V> {
        ComponentReader {
            entity: self.clone(),
            reader: value.into(),
        }
    }
    fn set(&self, value: impl Into<Writer<H, V>>) -> ComponentWriter<H, V> {
        ComponentWriter {
            entity: self.clone(),
            writer: value.into(),
        }
    }
}

pub struct BindComponentToComponent<S: Component, T: Component, V: Bindable> {
    pub from: ComponentReader<S, V>,
    pub to: ComponentWriter<T, V>,
}

impl<S: Component, T: Component, V: Bindable> Command for BindComponentToComponent<S, T, V> {
    fn apply(self, world: &mut World) {
        world.bind_component_to_component(self.from, self.to)
    }
}

pub struct BindResourceToComponent<S: Resource, T: Component, V: Bindable> {
    pub from: Reader<S, V>,
    pub to: ComponentWriter<T, V>,
}

impl<S: Resource, T: Component, V: Bindable> Command for BindResourceToComponent<S, T, V> {
    fn apply(self, world: &mut World) {
        world.bind_resource_to_component(self.from, self.to);
    }
}

pub struct ComponentReader<C: Component, V: Bindable> {
    entity: Entity,
    reader: Reader<C, V>,
}

pub enum Reader<H, V: Bindable> {
    Func(fn(&H) -> Value<V>),
    Closure(Rc<dyn Fn(&H) -> Value<V>>),
}

unsafe impl<H, V: Bindable> Send for Reader<H, V> {}
unsafe impl<H, V: Bindable> Sync for Reader<H, V> {}
impl<H, V: Bindable> Clone for Reader<H, V> {
    fn clone(&self) -> Self {
        match self {
            Self::Func(f) => Self::Func(f.clone()),
            Self::Closure(c) => Self::Closure(c.clone()),
        }
    }
}

impl<H, V: Bindable> From<Prop<H, V>> for Reader<H, V> {
    fn from(value: Prop<H, V>) -> Self {
        Reader::Func(value.getter())
    }
}
impl<H, V: Bindable> Reader<H, V> {
    pub fn read<'a>(&self, host: &'a H) -> Value<'a, V> {
        match self {
            Reader::Func(f) => f(host),
            Reader::Closure(c) => c(host),
        }
    }
}

pub struct ComponentWriter<C: Component, V: Bindable> {
    entity: Entity,
    writer: Writer<C, V>,
}

pub struct Writer<H, V> {
    get: fn(&H) -> Value<V>,
    set: fn(&mut H, V),
}
impl<H, V: Bindable> Writer<H, V> {
    pub fn read<'a>(&self, host: &'a H) -> Value<'a, V> {
        (self.get)(host)
    }
    pub fn write(&self, host: &mut H, value: V) {
        (self.set)(host, value)
    }
}
unsafe impl<H, V: Bindable> Send for Writer<H, V> {}
unsafe impl<H, V: Bindable> Sync for Writer<H, V> {}
impl<H, V: Bindable> Clone for Writer<H, V> {
    fn clone(&self) -> Self {
        Self {
            get: self.get.clone(),
            set: self.set.clone(),
        }
    }
}
impl<H, T: Bindable> From<Prop<H, T>> for Writer<H, T> {
    fn from(prop: Prop<H, T>) -> Self {
        Writer {
            get: prop.getter(),
            set: prop.setter(),
        }
    }
}

pub trait MapProp<H, V: Bindable> {
    fn map<F: Fn(&V) -> T + 'static, T: Bindable>(self, map: F) -> Reader<H, T>;
}

impl<H: 'static, V: Bindable> MapProp<H, V> for Prop<H, V> {
    fn map<F: Fn(&V) -> T + 'static, T: Bindable>(self, map: F) -> Reader<H, T> {
        Reader::Closure(Rc::new(move |host| {
            let val = self.get(host);
            Value::Val(map(val.as_ref()))
        }))
    }
}
