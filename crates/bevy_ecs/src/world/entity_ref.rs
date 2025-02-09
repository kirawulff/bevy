use crate::{
    archetype::{Archetype, ArchetypeId, Archetypes},
    bundle::{Bundle, BundleId, BundleInfo, BundleInserter, DynamicBundle, InsertMode},
    change_detection::MutUntyped,
    component::{Component, ComponentId, ComponentTicks, Components, Mutable, StorageType},
    entity::{
        Entities, Entity, EntityBorrow, EntityCloner, EntityClonerBuilder, EntityLocation,
        TrustedEntityBorrow,
    },
    event::Event,
    observer::Observer,
    query::{Access, ReadOnlyQueryData},
    removal_detection::RemovedComponentEvents,
    resource::Resource,
    storage::Storages,
    system::IntoObserverSystem,
    world::{
        error::EntityComponentError, unsafe_world_cell::UnsafeEntityCell, DeferredWorld, Mut, Ref,
        World, ON_DESPAWN, ON_REMOVE, ON_REPLACE,
    },
};
use alloc::vec::Vec;
use bevy_platform_support::collections::{HashMap, HashSet};
use bevy_ptr::{OwningPtr, Ptr};
#[cfg(feature = "track_location")]
use core::panic::Location;
use core::{
    any::TypeId,
    cmp::Ordering,
    hash::{Hash, Hasher},
    marker::PhantomData,
    mem::MaybeUninit,
};
use thiserror::Error;

/// A read-only reference to a particular [`Entity`] and all of its components.
///
/// # Examples
///
/// Read-only access disjoint with mutable access.
///
/// ```
/// # use bevy_ecs::prelude::*;
/// # #[derive(Component)] pub struct A;
/// # #[derive(Component)] pub struct B;
/// fn disjoint_system(
///     query1: Query<&mut A>,
///     query2: Query<EntityRef, Without<A>>,
/// ) {
///     // ...
/// }
/// # bevy_ecs::system::assert_is_system(disjoint_system);
/// ```
#[derive(Copy, Clone)]
pub struct EntityRef<'w> {
    cell: UnsafeEntityCell<'w>,
}

impl<'w> EntityRef<'w> {
    /// # Safety
    /// - `cell` must have permission to read every component of the entity.
    /// - No mutable accesses to any of the entity's components may exist
    ///   at the same time as the returned [`EntityRef`].
    #[inline]
    pub(crate) unsafe fn new(cell: UnsafeEntityCell<'w>) -> Self {
        Self { cell }
    }

    /// Returns the [ID](Entity) of the current entity.
    #[inline]
    #[must_use = "Omit the .id() call if you do not need to store the `Entity` identifier."]
    pub fn id(&self) -> Entity {
        self.cell.id()
    }

    /// Gets metadata indicating the location where the current entity is stored.
    #[inline]
    pub fn location(&self) -> EntityLocation {
        self.cell.location()
    }

    /// Returns the archetype that the current entity belongs to.
    #[inline]
    pub fn archetype(&self) -> &Archetype {
        self.cell.archetype()
    }

    /// Returns `true` if the current entity has a component of type `T`.
    /// Otherwise, this returns `false`.
    ///
    /// ## Notes
    ///
    /// If you do not know the concrete type of a component, consider using
    /// [`Self::contains_id`] or [`Self::contains_type_id`].
    #[inline]
    pub fn contains<T: Component>(&self) -> bool {
        self.contains_type_id(TypeId::of::<T>())
    }

    /// Returns `true` if the current entity has a component identified by `component_id`.
    /// Otherwise, this returns false.
    ///
    /// ## Notes
    ///
    /// - If you know the concrete type of the component, you should prefer [`Self::contains`].
    /// - If you know the component's [`TypeId`] but not its [`ComponentId`], consider using
    ///     [`Self::contains_type_id`].
    #[inline]
    pub fn contains_id(&self, component_id: ComponentId) -> bool {
        self.cell.contains_id(component_id)
    }

    /// Returns `true` if the current entity has a component with the type identified by `type_id`.
    /// Otherwise, this returns false.
    ///
    /// ## Notes
    ///
    /// - If you know the concrete type of the component, you should prefer [`Self::contains`].
    /// - If you have a [`ComponentId`] instead of a [`TypeId`], consider using [`Self::contains_id`].
    #[inline]
    pub fn contains_type_id(&self, type_id: TypeId) -> bool {
        self.cell.contains_type_id(type_id)
    }

    /// Gets access to the component of type `T` for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    #[inline]
    pub fn get<T: Component>(&self) -> Option<&'w T> {
        // SAFETY: We have read-only access to all components of this entity.
        unsafe { self.cell.get::<T>() }
    }

    /// Gets access to the component of type `T` for the current entity,
    /// including change detection information as a [`Ref`].
    ///
    /// Returns `None` if the entity does not have a component of type `T`.
    #[inline]
    pub fn get_ref<T: Component>(&self) -> Option<Ref<'w, T>> {
        // SAFETY: We have read-only access to all components of this entity.
        unsafe { self.cell.get_ref::<T>() }
    }

    /// Retrieves the change ticks for the given component. This can be useful for implementing change
    /// detection in custom runtimes.
    #[inline]
    pub fn get_change_ticks<T: Component>(&self) -> Option<ComponentTicks> {
        // SAFETY: We have read-only access to all components of this entity.
        unsafe { self.cell.get_change_ticks::<T>() }
    }

    /// Retrieves the change ticks for the given [`ComponentId`]. This can be useful for implementing change
    /// detection in custom runtimes.
    ///
    /// **You should prefer to use the typed API [`EntityRef::get_change_ticks`] where possible and only
    /// use this in cases where the actual component types are not known at
    /// compile time.**
    #[inline]
    pub fn get_change_ticks_by_id(&self, component_id: ComponentId) -> Option<ComponentTicks> {
        // SAFETY: We have read-only access to all components of this entity.
        unsafe { self.cell.get_change_ticks_by_id(component_id) }
    }

    /// Returns [untyped read-only reference(s)](Ptr) to component(s) for the
    /// current entity, based on the given [`ComponentId`]s.
    ///
    /// **You should prefer to use the typed API [`EntityRef::get`] where
    /// possible and only use this in cases where the actual component types
    /// are not known at compile time.**
    ///
    /// Unlike [`EntityRef::get`], this returns untyped reference(s) to
    /// component(s), and it's the job of the caller to ensure the correct
    /// type(s) are dereferenced (if necessary).
    ///
    /// # Errors
    ///
    /// Returns [`EntityComponentError::MissingComponent`] if the entity does
    /// not have a component.
    ///
    /// # Examples
    ///
    /// ## Single [`ComponentId`]
    ///
    /// ```
    /// # use bevy_ecs::prelude::*;
    /// #
    /// # #[derive(Component, PartialEq, Debug)]
    /// # pub struct Foo(i32);
    /// # let mut world = World::new();
    /// let entity = world.spawn(Foo(42)).id();
    ///
    /// // Grab the component ID for `Foo` in whatever way you like.
    /// let component_id = world.register_component::<Foo>();
    ///
    /// // Then, get the component by ID.
    /// let ptr = world.entity(entity).get_by_id(component_id);
    /// # assert_eq!(unsafe { ptr.unwrap().deref::<Foo>() }, &Foo(42));
    /// ```
    ///
    /// ## Array of [`ComponentId`]s
    ///
    /// ```
    /// # use bevy_ecs::prelude::*;
    /// #
    /// # #[derive(Component, PartialEq, Debug)]
    /// # pub struct X(i32);
    /// # #[derive(Component, PartialEq, Debug)]
    /// # pub struct Y(i32);
    /// # let mut world = World::new();
    /// let entity = world.spawn((X(42), Y(10))).id();
    ///
    /// // Grab the component IDs for `X` and `Y` in whatever way you like.
    /// let x_id = world.register_component::<X>();
    /// let y_id = world.register_component::<Y>();
    ///
    /// // Then, get the components by ID. You'll receive a same-sized array.
    /// let Ok([x_ptr, y_ptr]) = world.entity(entity).get_by_id([x_id, y_id]) else {
    ///     // Up to you to handle if a component is missing from the entity.
    /// #   unreachable!();
    /// };
    /// # assert_eq!((unsafe { x_ptr.deref::<X>() }, unsafe { y_ptr.deref::<Y>() }), (&X(42), &Y(10)));
    /// ```
    ///
    /// ## Slice of [`ComponentId`]s
    ///
    /// ```
    /// # use bevy_ecs::{prelude::*, component::ComponentId};
    /// #
    /// # #[derive(Component, PartialEq, Debug)]
    /// # pub struct X(i32);
    /// # #[derive(Component, PartialEq, Debug)]
    /// # pub struct Y(i32);
    /// # let mut world = World::new();
    /// let entity = world.spawn((X(42), Y(10))).id();
    ///
    /// // Grab the component IDs for `X` and `Y` in whatever way you like.
    /// let x_id = world.register_component::<X>();
    /// let y_id = world.register_component::<Y>();
    ///
    /// // Then, get the components by ID. You'll receive a vec of ptrs.
    /// let ptrs = world.entity(entity).get_by_id(&[x_id, y_id] as &[ComponentId]);
    /// # let ptrs = ptrs.unwrap();
    /// # assert_eq!((unsafe { ptrs[0].deref::<X>() }, unsafe { ptrs[1].deref::<Y>() }), (&X(42), &Y(10)));
    /// ```
    ///
    /// ## [`HashSet`] of [`ComponentId`]s
    ///
    /// ```
    /// # use bevy_platform_support::collections::HashSet;
    /// # use bevy_ecs::{prelude::*, component::ComponentId};
    /// #
    /// # #[derive(Component, PartialEq, Debug)]
    /// # pub struct X(i32);
    /// # #[derive(Component, PartialEq, Debug)]
    /// # pub struct Y(i32);
    /// # let mut world = World::new();
    /// let entity = world.spawn((X(42), Y(10))).id();
    ///
    /// // Grab the component IDs for `X` and `Y` in whatever way you like.
    /// let x_id = world.register_component::<X>();
    /// let y_id = world.register_component::<Y>();
    ///
    /// // Then, get the components by ID. You'll receive a vec of ptrs.
    /// let ptrs = world.entity(entity).get_by_id(&HashSet::from_iter([x_id, y_id]));
    /// # let ptrs = ptrs.unwrap();
    /// # assert_eq!((unsafe { ptrs[&x_id].deref::<X>() }, unsafe { ptrs[&y_id].deref::<Y>() }), (&X(42), &Y(10)));
    /// ```
    #[inline]
    pub fn get_by_id<F: DynamicComponentFetch>(
        &self,
        component_ids: F,
    ) -> Result<F::Ref<'w>, EntityComponentError> {
        // SAFETY: We have read-only access to all components of this entity.
        unsafe { component_ids.fetch_ref(self.cell) }
    }

    /// Returns read-only components for the current entity that match the query `Q`.
    ///
    /// # Panics
    ///
    /// If the entity does not have the components required by the query `Q`.
    pub fn components<Q: ReadOnlyQueryData>(&self) -> Q::Item<'w> {
        self.get_components::<Q>()
            .expect("Query does not match the current entity")
    }

    /// Returns read-only components for the current entity that match the query `Q`,
    /// or `None` if the entity does not have the components required by the query `Q`.
    pub fn get_components<Q: ReadOnlyQueryData>(&self) -> Option<Q::Item<'w>> {
        // SAFETY: We have read-only access to all components of this entity.
        unsafe { self.cell.get_components::<Q>() }
    }

    /// Returns the source code location from which this entity has been spawned.
    #[cfg(feature = "track_location")]
    pub fn spawned_by(&self) -> &'static Location<'static> {
        self.cell.spawned_by()
    }
}

impl<'w> From<EntityWorldMut<'w>> for EntityRef<'w> {
    fn from(entity: EntityWorldMut<'w>) -> EntityRef<'w> {
        // SAFETY:
        // - `EntityWorldMut` guarantees exclusive access to the entire world.
        unsafe { EntityRef::new(entity.into_unsafe_entity_cell()) }
    }
}

impl<'a> From<&'a EntityWorldMut<'_>> for EntityRef<'a> {
    fn from(entity: &'a EntityWorldMut<'_>) -> Self {
        // SAFETY:
        // - `EntityWorldMut` guarantees exclusive access to the entire world.
        // - `&entity` ensures no mutable accesses are active.
        unsafe { EntityRef::new(entity.as_unsafe_entity_cell_readonly()) }
    }
}

impl<'w> From<EntityMut<'w>> for EntityRef<'w> {
    fn from(entity: EntityMut<'w>) -> Self {
        // SAFETY:
        // - `EntityMut` guarantees exclusive access to all of the entity's components.
        unsafe { EntityRef::new(entity.cell) }
    }
}

impl<'a> From<&'a EntityMut<'_>> for EntityRef<'a> {
    fn from(entity: &'a EntityMut<'_>) -> Self {
        // SAFETY:
        // - `EntityMut` guarantees exclusive access to all of the entity's components.
        // - `&entity` ensures there are no mutable accesses.
        unsafe { EntityRef::new(entity.cell) }
    }
}

impl<'a> TryFrom<FilteredEntityRef<'a>> for EntityRef<'a> {
    type Error = TryFromFilteredError;

    fn try_from(entity: FilteredEntityRef<'a>) -> Result<Self, Self::Error> {
        if !entity.access.has_read_all() {
            Err(TryFromFilteredError::MissingReadAllAccess)
        } else {
            // SAFETY: check above guarantees read-only access to all components of the entity.
            Ok(unsafe { EntityRef::new(entity.entity) })
        }
    }
}

impl<'a> TryFrom<&'a FilteredEntityRef<'_>> for EntityRef<'a> {
    type Error = TryFromFilteredError;

    fn try_from(entity: &'a FilteredEntityRef<'_>) -> Result<Self, Self::Error> {
        if !entity.access.has_read_all() {
            Err(TryFromFilteredError::MissingReadAllAccess)
        } else {
            // SAFETY: check above guarantees read-only access to all components of the entity.
            Ok(unsafe { EntityRef::new(entity.entity) })
        }
    }
}

impl<'a> TryFrom<FilteredEntityMut<'a>> for EntityRef<'a> {
    type Error = TryFromFilteredError;

    fn try_from(entity: FilteredEntityMut<'a>) -> Result<Self, Self::Error> {
        if !entity.access.has_read_all() {
            Err(TryFromFilteredError::MissingReadAllAccess)
        } else {
            // SAFETY: check above guarantees read-only access to all components of the entity.
            Ok(unsafe { EntityRef::new(entity.entity) })
        }
    }
}

impl<'a> TryFrom<&'a FilteredEntityMut<'_>> for EntityRef<'a> {
    type Error = TryFromFilteredError;

    fn try_from(entity: &'a FilteredEntityMut<'_>) -> Result<Self, Self::Error> {
        if !entity.access.has_read_all() {
            Err(TryFromFilteredError::MissingReadAllAccess)
        } else {
            // SAFETY: check above guarantees read-only access to all components of the entity.
            Ok(unsafe { EntityRef::new(entity.entity) })
        }
    }
}

impl PartialEq for EntityRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.entity() == other.entity()
    }
}

impl Eq for EntityRef<'_> {}

impl PartialOrd for EntityRef<'_> {
    /// [`EntityRef`]'s comparison trait implementations match the underlying [`Entity`],
    /// and cannot discern between different worlds.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EntityRef<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.entity().cmp(&other.entity())
    }
}

impl Hash for EntityRef<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.entity().hash(state);
    }
}

impl EntityBorrow for EntityRef<'_> {
    fn entity(&self) -> Entity {
        self.id()
    }
}

// SAFETY: This type represents one Entity. We implement the comparison traits based on that Entity.
unsafe impl TrustedEntityBorrow for EntityRef<'_> {}

/// Provides mutable access to a single entity and all of its components.
///
/// Contrast with [`EntityWorldMut`], which allows adding and removing components,
/// despawning the entity, and provides mutable access to the entire world.
/// Because of this, `EntityWorldMut` cannot coexist with any other world accesses.
///
/// # Examples
///
/// Disjoint mutable access.
///
/// ```
/// # use bevy_ecs::prelude::*;
/// # #[derive(Component)] pub struct A;
/// fn disjoint_system(
///     query1: Query<EntityMut, With<A>>,
///     query2: Query<EntityMut, Without<A>>,
/// ) {
///     // ...
/// }
/// # bevy_ecs::system::assert_is_system(disjoint_system);
/// ```
pub struct EntityMut<'w> {
    cell: UnsafeEntityCell<'w>,
}

impl<'w> EntityMut<'w> {
    /// # Safety
    /// - `cell` must have permission to mutate every component of the entity.
    /// - No accesses to any of the entity's components may exist
    ///   at the same time as the returned [`EntityMut`].
    pub(crate) unsafe fn new(cell: UnsafeEntityCell<'w>) -> Self {
        Self { cell }
    }

    /// Returns a new instance with a shorter lifetime.
    /// This is useful if you have `&mut EntityMut`, but you need `EntityMut`.
    pub fn reborrow(&mut self) -> EntityMut<'_> {
        // SAFETY: We have exclusive access to the entire entity and its components.
        unsafe { Self::new(self.cell) }
    }

    /// Consumes `self` and returns read-only access to all of the entity's
    /// components, with the world `'w` lifetime.
    pub fn into_readonly(self) -> EntityRef<'w> {
        EntityRef::from(self)
    }

    /// Gets read-only access to all of the entity's components.
    pub fn as_readonly(&self) -> EntityRef<'_> {
        EntityRef::from(self)
    }

    /// Returns the [ID](Entity) of the current entity.
    #[inline]
    #[must_use = "Omit the .id() call if you do not need to store the `Entity` identifier."]
    pub fn id(&self) -> Entity {
        self.cell.id()
    }

    /// Gets metadata indicating the location where the current entity is stored.
    #[inline]
    pub fn location(&self) -> EntityLocation {
        self.cell.location()
    }

    /// Returns the archetype that the current entity belongs to.
    #[inline]
    pub fn archetype(&self) -> &Archetype {
        self.cell.archetype()
    }

    /// Returns `true` if the current entity has a component of type `T`.
    /// Otherwise, this returns `false`.
    ///
    /// ## Notes
    ///
    /// If you do not know the concrete type of a component, consider using
    /// [`Self::contains_id`] or [`Self::contains_type_id`].
    #[inline]
    pub fn contains<T: Component>(&self) -> bool {
        self.contains_type_id(TypeId::of::<T>())
    }

    /// Returns `true` if the current entity has a component identified by `component_id`.
    /// Otherwise, this returns false.
    ///
    /// ## Notes
    ///
    /// - If you know the concrete type of the component, you should prefer [`Self::contains`].
    /// - If you know the component's [`TypeId`] but not its [`ComponentId`], consider using
    ///     [`Self::contains_type_id`].
    #[inline]
    pub fn contains_id(&self, component_id: ComponentId) -> bool {
        self.cell.contains_id(component_id)
    }

    /// Returns `true` if the current entity has a component with the type identified by `type_id`.
    /// Otherwise, this returns false.
    ///
    /// ## Notes
    ///
    /// - If you know the concrete type of the component, you should prefer [`Self::contains`].
    /// - If you have a [`ComponentId`] instead of a [`TypeId`], consider using [`Self::contains_id`].
    #[inline]
    pub fn contains_type_id(&self, type_id: TypeId) -> bool {
        self.cell.contains_type_id(type_id)
    }

    /// Gets access to the component of type `T` for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    #[inline]
    pub fn get<T: Component>(&self) -> Option<&'_ T> {
        self.as_readonly().get()
    }

    /// Returns read-only components for the current entity that match the query `Q`.
    ///
    /// # Panics
    ///
    /// If the entity does not have the components required by the query `Q`.
    pub fn components<Q: ReadOnlyQueryData>(&self) -> Q::Item<'_> {
        self.as_readonly().components::<Q>()
    }

    /// Returns read-only components for the current entity that match the query `Q`,
    /// or `None` if the entity does not have the components required by the query `Q`.
    pub fn get_components<Q: ReadOnlyQueryData>(&self) -> Option<Q::Item<'_>> {
        self.as_readonly().get_components::<Q>()
    }

    /// Consumes `self` and gets access to the component of type `T` with the
    /// world `'w` lifetime for the current entity.
    ///
    /// Returns `None` if the entity does not have a component of type `T`.
    #[inline]
    pub fn into_borrow<T: Component>(self) -> Option<&'w T> {
        self.into_readonly().get()
    }

    /// Gets access to the component of type `T` for the current entity,
    /// including change detection information as a [`Ref`].
    ///
    /// Returns `None` if the entity does not have a component of type `T`.
    #[inline]
    pub fn get_ref<T: Component>(&self) -> Option<Ref<'_, T>> {
        self.as_readonly().get_ref()
    }

    /// Consumes `self` and gets access to the component of type `T` with world
    /// `'w` lifetime for the current entity, including change detection information
    /// as a [`Ref<'w>`].
    ///
    /// Returns `None` if the entity does not have a component of type `T`.
    #[inline]
    pub fn into_ref<T: Component>(self) -> Option<Ref<'w, T>> {
        self.into_readonly().get_ref()
    }

    /// Gets mutable access to the component of type `T` for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    #[inline]
    pub fn get_mut<T: Component<Mutability = Mutable>>(&mut self) -> Option<Mut<'_, T>> {
        // SAFETY: &mut self implies exclusive access for duration of returned value
        unsafe { self.cell.get_mut() }
    }

    /// Gets mutable access to the component of type `T` for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    ///
    /// # Safety
    ///
    /// - `T` must be a mutable component
    #[inline]
    pub unsafe fn get_mut_assume_mutable<T: Component>(&mut self) -> Option<Mut<'_, T>> {
        // SAFETY:
        // - &mut self implies exclusive access for duration of returned value
        // - Caller ensures `T` is a mutable component
        unsafe { self.cell.get_mut_assume_mutable() }
    }

    /// Consumes self and gets mutable access to the component of type `T`
    /// with the world `'w` lifetime for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    #[inline]
    pub fn into_mut<T: Component<Mutability = Mutable>>(self) -> Option<Mut<'w, T>> {
        // SAFETY: consuming `self` implies exclusive access
        unsafe { self.cell.get_mut() }
    }

    /// Gets mutable access to the component of type `T` for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    ///
    /// # Safety
    ///
    /// - `T` must be a mutable component
    #[inline]
    pub unsafe fn into_mut_assume_mutable<T: Component>(self) -> Option<Mut<'w, T>> {
        // SAFETY:
        // - Consuming `self` implies exclusive access
        // - Caller ensures `T` is a mutable component
        unsafe { self.cell.get_mut_assume_mutable() }
    }

    /// Retrieves the change ticks for the given component. This can be useful for implementing change
    /// detection in custom runtimes.
    #[inline]
    pub fn get_change_ticks<T: Component>(&self) -> Option<ComponentTicks> {
        self.as_readonly().get_change_ticks::<T>()
    }

    /// Retrieves the change ticks for the given [`ComponentId`]. This can be useful for implementing change
    /// detection in custom runtimes.
    ///
    /// **You should prefer to use the typed API [`EntityWorldMut::get_change_ticks`] where possible and only
    /// use this in cases where the actual component types are not known at
    /// compile time.**
    #[inline]
    pub fn get_change_ticks_by_id(&self, component_id: ComponentId) -> Option<ComponentTicks> {
        self.as_readonly().get_change_ticks_by_id(component_id)
    }

    /// Returns [untyped read-only reference(s)](Ptr) to component(s) for the
    /// current entity, based on the given [`ComponentId`]s.
    ///
    /// **You should prefer to use the typed API [`EntityMut::get`] where
    /// possible and only use this in cases where the actual component types
    /// are not known at compile time.**
    ///
    /// Unlike [`EntityMut::get`], this returns untyped reference(s) to
    /// component(s), and it's the job of the caller to ensure the correct
    /// type(s) are dereferenced (if necessary).
    ///
    /// # Errors
    ///
    /// Returns [`EntityComponentError::MissingComponent`] if the entity does
    /// not have a component.
    ///
    /// # Examples
    ///
    /// For examples on how to use this method, see [`EntityRef::get_by_id`].
    #[inline]
    pub fn get_by_id<F: DynamicComponentFetch>(
        &self,
        component_ids: F,
    ) -> Result<F::Ref<'_>, EntityComponentError> {
        self.as_readonly().get_by_id(component_ids)
    }

    /// Consumes `self` and returns [untyped read-only reference(s)](Ptr) to
    /// component(s) with lifetime `'w` for the current entity, based on the
    /// given [`ComponentId`]s.
    ///
    /// **You should prefer to use the typed API [`EntityMut::into_borrow`]
    /// where possible and only use this in cases where the actual component
    /// types are not known at compile time.**
    ///
    /// Unlike [`EntityMut::into_borrow`], this returns untyped reference(s) to
    /// component(s), and it's the job of the caller to ensure the correct
    /// type(s) are dereferenced (if necessary).
    ///
    /// # Errors
    ///
    /// Returns [`EntityComponentError::MissingComponent`] if the entity does
    /// not have a component.
    ///
    /// # Examples
    ///
    /// For examples on how to use this method, see [`EntityRef::get_by_id`].
    #[inline]
    pub fn into_borrow_by_id<F: DynamicComponentFetch>(
        self,
        component_ids: F,
    ) -> Result<F::Ref<'w>, EntityComponentError> {
        self.into_readonly().get_by_id(component_ids)
    }

    /// Returns [untyped mutable reference(s)](MutUntyped) to component(s) for
    /// the current entity, based on the given [`ComponentId`]s.
    ///
    /// **You should prefer to use the typed API [`EntityMut::get_mut`] where
    /// possible and only use this in cases where the actual component types
    /// are not known at compile time.**
    ///
    /// Unlike [`EntityMut::get_mut`], this returns untyped reference(s) to
    /// component(s), and it's the job of the caller to ensure the correct
    /// type(s) are dereferenced (if necessary).
    ///
    /// # Errors
    ///
    /// - Returns [`EntityComponentError::MissingComponent`] if the entity does
    ///   not have a component.
    /// - Returns [`EntityComponentError::AliasedMutability`] if a component
    ///   is requested multiple times.
    ///
    /// # Examples
    ///
    /// ## Single [`ComponentId`]
    ///
    /// ```
    /// # use bevy_ecs::prelude::*;
    /// #
    /// # #[derive(Component, PartialEq, Debug)]
    /// # pub struct Foo(i32);
    /// # let mut world = World::new();
    /// let entity = world.spawn(Foo(42)).id();
    ///
    /// // Grab the component ID for `Foo` in whatever way you like.
    /// let component_id = world.register_component::<Foo>();
    ///
    /// // Then, get the component by ID.
    /// let mut entity_mut = world.entity_mut(entity);
    /// let mut ptr = entity_mut.get_mut_by_id(component_id)
    /// #   .unwrap();
    /// # assert_eq!(unsafe { ptr.as_mut().deref_mut::<Foo>() }, &mut Foo(42));
    /// ```
    ///
    /// ## Array of [`ComponentId`]s
    ///
    /// ```
    /// # use bevy_ecs::prelude::*;
    /// #
    /// # #[derive(Component, PartialEq, Debug)]
    /// # pub struct X(i32);
    /// # #[derive(Component, PartialEq, Debug)]
    /// # pub struct Y(i32);
    /// # let mut world = World::new();
    /// let entity = world.spawn((X(42), Y(10))).id();
    ///
    /// // Grab the component IDs for `X` and `Y` in whatever way you like.
    /// let x_id = world.register_component::<X>();
    /// let y_id = world.register_component::<Y>();
    ///
    /// // Then, get the components by ID. You'll receive a same-sized array.
    /// let mut entity_mut = world.entity_mut(entity);
    /// let Ok([mut x_ptr, mut y_ptr]) = entity_mut.get_mut_by_id([x_id, y_id]) else {
    ///     // Up to you to handle if a component is missing from the entity.
    /// #   unreachable!();
    /// };
    /// # assert_eq!((unsafe { x_ptr.as_mut().deref_mut::<X>() }, unsafe { y_ptr.as_mut().deref_mut::<Y>() }), (&mut X(42), &mut Y(10)));
    /// ```
    ///
    /// ## Slice of [`ComponentId`]s
    ///
    /// ```
    /// # use bevy_ecs::{prelude::*, component::ComponentId, change_detection::MutUntyped};
    /// #
    /// # #[derive(Component, PartialEq, Debug)]
    /// # pub struct X(i32);
    /// # #[derive(Component, PartialEq, Debug)]
    /// # pub struct Y(i32);
    /// # let mut world = World::new();
    /// let entity = world.spawn((X(42), Y(10))).id();
    ///
    /// // Grab the component IDs for `X` and `Y` in whatever way you like.
    /// let x_id = world.register_component::<X>();
    /// let y_id = world.register_component::<Y>();
    ///
    /// // Then, get the components by ID. You'll receive a vec of ptrs.
    /// let mut entity_mut = world.entity_mut(entity);
    /// let ptrs = entity_mut.get_mut_by_id(&[x_id, y_id] as &[ComponentId])
    /// #   .unwrap();
    /// # let [mut x_ptr, mut y_ptr]: [MutUntyped; 2] = ptrs.try_into().unwrap();
    /// # assert_eq!((unsafe { x_ptr.as_mut().deref_mut::<X>() }, unsafe { y_ptr.as_mut().deref_mut::<Y>() }), (&mut X(42), &mut Y(10)));
    /// ```
    ///
    /// ## [`HashSet`] of [`ComponentId`]s
    ///
    /// ```
    /// # use bevy_platform_support::collections::HashSet;
    /// # use bevy_ecs::{prelude::*, component::ComponentId};
    /// #
    /// # #[derive(Component, PartialEq, Debug)]
    /// # pub struct X(i32);
    /// # #[derive(Component, PartialEq, Debug)]
    /// # pub struct Y(i32);
    /// # let mut world = World::new();
    /// let entity = world.spawn((X(42), Y(10))).id();
    ///
    /// // Grab the component IDs for `X` and `Y` in whatever way you like.
    /// let x_id = world.register_component::<X>();
    /// let y_id = world.register_component::<Y>();
    ///
    /// // Then, get the components by ID. You'll receive a `HashMap` of ptrs.
    /// let mut entity_mut = world.entity_mut(entity);
    /// let mut ptrs = entity_mut.get_mut_by_id(&HashSet::from_iter([x_id, y_id]))
    /// #   .unwrap();
    /// # let [Some(mut x_ptr), Some(mut y_ptr)] = ptrs.get_many_mut([&x_id, &y_id]) else { unreachable!() };
    /// # assert_eq!((unsafe { x_ptr.as_mut().deref_mut::<X>() }, unsafe { y_ptr.as_mut().deref_mut::<Y>() }), (&mut X(42), &mut Y(10)));
    /// ```
    #[inline]
    pub fn get_mut_by_id<F: DynamicComponentFetch>(
        &mut self,
        component_ids: F,
    ) -> Result<F::Mut<'_>, EntityComponentError> {
        // SAFETY:
        // - `&mut self` ensures that no references exist to this entity's components.
        // - We have exclusive access to all components of this entity.
        unsafe { component_ids.fetch_mut(self.cell) }
    }

    /// Returns [untyped mutable reference](MutUntyped) to component for
    /// the current entity, based on the given [`ComponentId`].
    ///
    /// Unlike [`EntityMut::get_mut_by_id`], this method borrows &self instead of
    /// &mut self, allowing the caller to access multiple components simultaneously.
    ///
    /// # Errors
    ///
    /// - Returns [`EntityComponentError::MissingComponent`] if the entity does
    ///   not have a component.
    /// - Returns [`EntityComponentError::AliasedMutability`] if a component
    ///   is requested multiple times.
    ///
    /// # Safety
    /// It is the callers responsibility to ensure that
    /// - the [`UnsafeEntityCell`] has permission to access the component mutably
    /// - no other references to the component exist at the same time
    #[inline]
    pub unsafe fn get_mut_by_id_unchecked<F: DynamicComponentFetch>(
        &self,
        component_ids: F,
    ) -> Result<F::Mut<'_>, EntityComponentError> {
        // SAFETY:
        // - The caller must ensure simultaneous access is limited
        // - to components that are mutually independent.
        unsafe { component_ids.fetch_mut(self.cell) }
    }

    /// Consumes `self` and returns [untyped mutable reference(s)](MutUntyped)
    /// to component(s) with lifetime `'w` for the current entity, based on the
    /// given [`ComponentId`]s.
    ///
    /// **You should prefer to use the typed API [`EntityMut::into_mut`] where
    /// possible and only use this in cases where the actual component types
    /// are not known at compile time.**
    ///
    /// Unlike [`EntityMut::into_mut`], this returns untyped reference(s) to
    /// component(s), and it's the job of the caller to ensure the correct
    /// type(s) are dereferenced (if necessary).
    ///
    /// # Errors
    ///
    /// - Returns [`EntityComponentError::MissingComponent`] if the entity does
    ///   not have a component.
    /// - Returns [`EntityComponentError::AliasedMutability`] if a component
    ///   is requested multiple times.
    ///
    /// # Examples
    ///
    /// For examples on how to use this method, see [`EntityMut::get_mut_by_id`].
    #[inline]
    pub fn into_mut_by_id<F: DynamicComponentFetch>(
        self,
        component_ids: F,
    ) -> Result<F::Mut<'w>, EntityComponentError> {
        // SAFETY:
        // - consuming `self` ensures that no references exist to this entity's components.
        // - We have exclusive access to all components of this entity.
        unsafe { component_ids.fetch_mut(self.cell) }
    }

    /// Returns the source code location from which this entity has been spawned.
    #[cfg(feature = "track_location")]
    pub fn spawned_by(&self) -> &'static Location<'static> {
        self.cell.spawned_by()
    }
}

impl<'w> From<&'w mut EntityMut<'_>> for EntityMut<'w> {
    fn from(entity: &'w mut EntityMut<'_>) -> Self {
        entity.reborrow()
    }
}

impl<'w> From<EntityWorldMut<'w>> for EntityMut<'w> {
    fn from(entity: EntityWorldMut<'w>) -> Self {
        // SAFETY: `EntityWorldMut` guarantees exclusive access to the entire world.
        unsafe { EntityMut::new(entity.into_unsafe_entity_cell()) }
    }
}

impl<'a> From<&'a mut EntityWorldMut<'_>> for EntityMut<'a> {
    fn from(entity: &'a mut EntityWorldMut<'_>) -> Self {
        // SAFETY: `EntityWorldMut` guarantees exclusive access to the entire world.
        unsafe { EntityMut::new(entity.as_unsafe_entity_cell()) }
    }
}

impl<'a> TryFrom<FilteredEntityMut<'a>> for EntityMut<'a> {
    type Error = TryFromFilteredError;

    fn try_from(entity: FilteredEntityMut<'a>) -> Result<Self, Self::Error> {
        if !entity.access.has_read_all() {
            Err(TryFromFilteredError::MissingReadAllAccess)
        } else if !entity.access.has_write_all() {
            Err(TryFromFilteredError::MissingWriteAllAccess)
        } else {
            // SAFETY: check above guarantees exclusive access to all components of the entity.
            Ok(unsafe { EntityMut::new(entity.entity) })
        }
    }
}

impl<'a> TryFrom<&'a mut FilteredEntityMut<'_>> for EntityMut<'a> {
    type Error = TryFromFilteredError;

    fn try_from(entity: &'a mut FilteredEntityMut<'_>) -> Result<Self, Self::Error> {
        if !entity.access.has_read_all() {
            Err(TryFromFilteredError::MissingReadAllAccess)
        } else if !entity.access.has_write_all() {
            Err(TryFromFilteredError::MissingWriteAllAccess)
        } else {
            // SAFETY: check above guarantees exclusive access to all components of the entity.
            Ok(unsafe { EntityMut::new(entity.entity) })
        }
    }
}

impl PartialEq for EntityMut<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.entity() == other.entity()
    }
}

impl Eq for EntityMut<'_> {}

impl PartialOrd for EntityMut<'_> {
    /// [`EntityMut`]'s comparison trait implementations match the underlying [`Entity`],
    /// and cannot discern between different worlds.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EntityMut<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.entity().cmp(&other.entity())
    }
}

impl Hash for EntityMut<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.entity().hash(state);
    }
}

impl EntityBorrow for EntityMut<'_> {
    fn entity(&self) -> Entity {
        self.id()
    }
}

// SAFETY: This type represents one Entity. We implement the comparison traits based on that Entity.
unsafe impl TrustedEntityBorrow for EntityMut<'_> {}

/// A mutable reference to a particular [`Entity`], and the entire world.
///
/// This is essentially a performance-optimized `(Entity, &mut World)` tuple,
/// which caches the [`EntityLocation`] to reduce duplicate lookups.
///
/// Since this type provides mutable access to the entire world, only one
/// [`EntityWorldMut`] can exist at a time for a given world.
///
/// See also [`EntityMut`], which allows disjoint mutable access to multiple
/// entities at once.  Unlike `EntityMut`, this type allows adding and
/// removing components, and despawning the entity.
pub struct EntityWorldMut<'w> {
    world: &'w mut World,
    entity: Entity,
    location: EntityLocation,
}

impl<'w> EntityWorldMut<'w> {
    #[track_caller]
    #[inline(never)]
    #[cold]
    fn panic_despawned(&self) -> ! {
        panic!(
            "Entity {} {}",
            self.entity,
            self.world
                .entities()
                .entity_does_not_exist_error_details(self.entity)
        );
    }

    #[inline(always)]
    #[track_caller]
    pub(crate) fn assert_not_despawned(&self) {
        if self.location.archetype_id == ArchetypeId::INVALID {
            self.panic_despawned();
        }
    }

    fn as_unsafe_entity_cell_readonly(&self) -> UnsafeEntityCell<'_> {
        self.assert_not_despawned();
        UnsafeEntityCell::new(
            self.world.as_unsafe_world_cell_readonly(),
            self.entity,
            self.location,
        )
    }
    fn as_unsafe_entity_cell(&mut self) -> UnsafeEntityCell<'_> {
        self.assert_not_despawned();
        UnsafeEntityCell::new(
            self.world.as_unsafe_world_cell(),
            self.entity,
            self.location,
        )
    }
    fn into_unsafe_entity_cell(self) -> UnsafeEntityCell<'w> {
        self.assert_not_despawned();
        UnsafeEntityCell::new(
            self.world.as_unsafe_world_cell(),
            self.entity,
            self.location,
        )
    }

    /// # Safety
    ///
    ///  - `entity` must be valid for `world`: the generation should match that of the entity at the same index.
    ///  - `location` must be sourced from `world`'s `Entities` and must exactly match the location for `entity`
    ///
    ///  The above is trivially satisfied if `location` was sourced from `world.entities().get(entity)`.
    #[inline]
    pub(crate) unsafe fn new(
        world: &'w mut World,
        entity: Entity,
        location: EntityLocation,
    ) -> Self {
        debug_assert!(world.entities().contains(entity));
        debug_assert_eq!(world.entities().get(entity), Some(location));

        EntityWorldMut {
            world,
            entity,
            location,
        }
    }

    /// Consumes `self` and returns read-only access to all of the entity's
    /// components, with the world `'w` lifetime.
    pub fn into_readonly(self) -> EntityRef<'w> {
        EntityRef::from(self)
    }

    /// Gets read-only access to all of the entity's components.
    pub fn as_readonly(&self) -> EntityRef<'_> {
        EntityRef::from(self)
    }

    /// Consumes `self` and returns non-structural mutable access to all of the
    /// entity's components, with the world `'w` lifetime.
    pub fn into_mutable(self) -> EntityMut<'w> {
        EntityMut::from(self)
    }

    /// Gets non-structural mutable access to all of the entity's components.
    pub fn as_mutable(&mut self) -> EntityMut<'_> {
        EntityMut::from(self)
    }

    /// Returns the [ID](Entity) of the current entity.
    #[inline]
    #[must_use = "Omit the .id() call if you do not need to store the `Entity` identifier."]
    pub fn id(&self) -> Entity {
        self.entity
    }

    /// Gets metadata indicating the location where the current entity is stored.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn location(&self) -> EntityLocation {
        self.assert_not_despawned();
        self.location
    }

    /// Returns the archetype that the current entity belongs to.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn archetype(&self) -> &Archetype {
        self.assert_not_despawned();
        &self.world.archetypes[self.location.archetype_id]
    }

    /// Returns `true` if the current entity has a component of type `T`.
    /// Otherwise, this returns `false`.
    ///
    /// ## Notes
    ///
    /// If you do not know the concrete type of a component, consider using
    /// [`Self::contains_id`] or [`Self::contains_type_id`].
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn contains<T: Component>(&self) -> bool {
        self.contains_type_id(TypeId::of::<T>())
    }

    /// Returns `true` if the current entity has a component identified by `component_id`.
    /// Otherwise, this returns false.
    ///
    /// ## Notes
    ///
    /// - If you know the concrete type of the component, you should prefer [`Self::contains`].
    /// - If you know the component's [`TypeId`] but not its [`ComponentId`], consider using
    ///     [`Self::contains_type_id`].
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn contains_id(&self, component_id: ComponentId) -> bool {
        self.as_unsafe_entity_cell_readonly()
            .contains_id(component_id)
    }

    /// Returns `true` if the current entity has a component with the type identified by `type_id`.
    /// Otherwise, this returns false.
    ///
    /// ## Notes
    ///
    /// - If you know the concrete type of the component, you should prefer [`Self::contains`].
    /// - If you have a [`ComponentId`] instead of a [`TypeId`], consider using [`Self::contains_id`].
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn contains_type_id(&self, type_id: TypeId) -> bool {
        self.as_unsafe_entity_cell_readonly()
            .contains_type_id(type_id)
    }

    /// Gets access to the component of type `T` for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn get<T: Component>(&self) -> Option<&'_ T> {
        self.as_readonly().get()
    }

    /// Returns read-only components for the current entity that match the query `Q`.
    ///
    /// # Panics
    ///
    /// If the entity does not have the components required by the query `Q` or if the entity
    /// has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn components<Q: ReadOnlyQueryData>(&self) -> Q::Item<'_> {
        self.as_readonly().components::<Q>()
    }

    /// Returns read-only components for the current entity that match the query `Q`,
    /// or `None` if the entity does not have the components required by the query `Q`.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn get_components<Q: ReadOnlyQueryData>(&self) -> Option<Q::Item<'_>> {
        self.as_readonly().get_components::<Q>()
    }

    /// Consumes `self` and gets access to the component of type `T` with
    /// the world `'w` lifetime for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn into_borrow<T: Component>(self) -> Option<&'w T> {
        self.into_readonly().get()
    }

    /// Gets access to the component of type `T` for the current entity,
    /// including change detection information as a [`Ref`].
    ///
    /// Returns `None` if the entity does not have a component of type `T`.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn get_ref<T: Component>(&self) -> Option<Ref<'_, T>> {
        self.as_readonly().get_ref()
    }

    /// Consumes `self` and gets access to the component of type `T`
    /// with the world `'w` lifetime for the current entity,
    /// including change detection information as a [`Ref`].
    ///
    /// Returns `None` if the entity does not have a component of type `T`.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn into_ref<T: Component>(self) -> Option<Ref<'w, T>> {
        self.into_readonly().get_ref()
    }

    /// Gets mutable access to the component of type `T` for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn get_mut<T: Component<Mutability = Mutable>>(&mut self) -> Option<Mut<'_, T>> {
        self.as_mutable().into_mut()
    }

    /// Temporarily removes a [`Component`] `T` from this [`Entity`] and runs the
    /// provided closure on it, returning the result if `T` was available.
    /// This will trigger the `OnRemove` and `OnReplace` component hooks without
    /// causing an archetype move.
    ///
    /// This is most useful with immutable components, where removal and reinsertion
    /// is the only way to modify a value.
    ///
    /// If you do not need to ensure the above hooks are triggered, and your component
    /// is mutable, prefer using [`get_mut`](EntityWorldMut::get_mut).
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use bevy_ecs::prelude::*;
    /// #
    /// #[derive(Component, PartialEq, Eq, Debug)]
    /// #[component(immutable)]
    /// struct Foo(bool);
    ///
    /// # let mut world = World::default();
    /// # world.register_component::<Foo>();
    /// #
    /// # let entity = world.spawn(Foo(false)).id();
    /// #
    /// # let mut entity = world.entity_mut(entity);
    /// #
    /// # assert_eq!(entity.get::<Foo>(), Some(&Foo(false)));
    /// #
    /// entity.modify_component(|foo: &mut Foo| {
    ///     foo.0 = true;
    /// });
    /// #
    /// # assert_eq!(entity.get::<Foo>(), Some(&Foo(true)));
    /// ```
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn modify_component<T: Component, R>(&mut self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        self.assert_not_despawned();

        let result = self
            .world
            .modify_component(self.entity, f)
            .expect("entity access must be valid")?;

        self.update_location();

        Some(result)
    }

    /// Gets mutable access to the component of type `T` for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    ///
    /// # Safety
    ///
    /// - `T` must be a mutable component
    #[inline]
    pub unsafe fn get_mut_assume_mutable<T: Component>(&mut self) -> Option<Mut<'_, T>> {
        self.as_mutable().into_mut_assume_mutable()
    }

    /// Consumes `self` and gets mutable access to the component of type `T`
    /// with the world `'w` lifetime for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn into_mut<T: Component<Mutability = Mutable>>(self) -> Option<Mut<'w, T>> {
        // SAFETY: consuming `self` implies exclusive access
        unsafe { self.into_unsafe_entity_cell().get_mut() }
    }

    /// Gets a reference to the resource of the given type
    ///
    /// # Panics
    ///
    /// Panics if the resource does not exist.
    /// Use [`get_resource`](EntityWorldMut::get_resource) instead if you want to handle this case.
    #[inline]
    #[track_caller]
    pub fn resource<R: Resource>(&self) -> &R {
        self.world.resource::<R>()
    }

    /// Gets a mutable reference to the resource of the given type
    ///
    /// # Panics
    ///
    /// Panics if the resource does not exist.
    /// Use [`get_resource_mut`](World::get_resource_mut) instead if you want to handle this case.
    ///
    /// If you want to instead insert a value if the resource does not exist,
    /// use [`get_resource_or_insert_with`](World::get_resource_or_insert_with).
    #[inline]
    #[track_caller]
    pub fn resource_mut<R: Resource>(&mut self) -> Mut<'_, R> {
        self.world.resource_mut::<R>()
    }

    /// Gets a reference to the resource of the given type if it exists
    #[inline]
    pub fn get_resource<R: Resource>(&self) -> Option<&R> {
        self.world.get_resource()
    }

    /// Gets a mutable reference to the resource of the given type if it exists
    #[inline]
    pub fn get_resource_mut<R: Resource>(&mut self) -> Option<Mut<'_, R>> {
        self.world.get_resource_mut()
    }

    /// Retrieves the change ticks for the given component. This can be useful for implementing change
    /// detection in custom runtimes.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn get_change_ticks<T: Component>(&self) -> Option<ComponentTicks> {
        self.as_readonly().get_change_ticks::<T>()
    }

    /// Retrieves the change ticks for the given [`ComponentId`]. This can be useful for implementing change
    /// detection in custom runtimes.
    ///
    /// **You should prefer to use the typed API [`EntityWorldMut::get_change_ticks`] where possible and only
    /// use this in cases where the actual component types are not known at
    /// compile time.**
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn get_change_ticks_by_id(&self, component_id: ComponentId) -> Option<ComponentTicks> {
        self.as_readonly().get_change_ticks_by_id(component_id)
    }

    /// Returns [untyped read-only reference(s)](Ptr) to component(s) for the
    /// current entity, based on the given [`ComponentId`]s.
    ///
    /// **You should prefer to use the typed API [`EntityWorldMut::get`] where
    /// possible and only use this in cases where the actual component types
    /// are not known at compile time.**
    ///
    /// Unlike [`EntityWorldMut::get`], this returns untyped reference(s) to
    /// component(s), and it's the job of the caller to ensure the correct
    /// type(s) are dereferenced (if necessary).
    ///
    /// # Errors
    ///
    /// Returns [`EntityComponentError::MissingComponent`] if the entity does
    /// not have a component.
    ///
    /// # Examples
    ///
    /// For examples on how to use this method, see [`EntityRef::get_by_id`].
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn get_by_id<F: DynamicComponentFetch>(
        &self,
        component_ids: F,
    ) -> Result<F::Ref<'_>, EntityComponentError> {
        self.as_readonly().get_by_id(component_ids)
    }

    /// Consumes `self` and returns [untyped read-only reference(s)](Ptr) to
    /// component(s) with lifetime `'w` for the current entity, based on the
    /// given [`ComponentId`]s.
    ///
    /// **You should prefer to use the typed API [`EntityWorldMut::into_borrow`]
    /// where possible and only use this in cases where the actual component
    /// types are not known at compile time.**
    ///
    /// Unlike [`EntityWorldMut::into_borrow`], this returns untyped reference(s) to
    /// component(s), and it's the job of the caller to ensure the correct
    /// type(s) are dereferenced (if necessary).
    ///
    /// # Errors
    ///
    /// Returns [`EntityComponentError::MissingComponent`] if the entity does
    /// not have a component.
    ///
    /// # Examples
    ///
    /// For examples on how to use this method, see [`EntityRef::get_by_id`].
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn into_borrow_by_id<F: DynamicComponentFetch>(
        self,
        component_ids: F,
    ) -> Result<F::Ref<'w>, EntityComponentError> {
        self.into_readonly().get_by_id(component_ids)
    }

    /// Returns [untyped mutable reference(s)](MutUntyped) to component(s) for
    /// the current entity, based on the given [`ComponentId`]s.
    ///
    /// **You should prefer to use the typed API [`EntityWorldMut::get_mut`] where
    /// possible and only use this in cases where the actual component types
    /// are not known at compile time.**
    ///
    /// Unlike [`EntityWorldMut::get_mut`], this returns untyped reference(s) to
    /// component(s), and it's the job of the caller to ensure the correct
    /// type(s) are dereferenced (if necessary).
    ///
    /// # Errors
    ///
    /// - Returns [`EntityComponentError::MissingComponent`] if the entity does
    ///   not have a component.
    /// - Returns [`EntityComponentError::AliasedMutability`] if a component
    ///   is requested multiple times.
    ///
    /// # Examples
    ///
    /// For examples on how to use this method, see [`EntityMut::get_mut_by_id`].
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn get_mut_by_id<F: DynamicComponentFetch>(
        &mut self,
        component_ids: F,
    ) -> Result<F::Mut<'_>, EntityComponentError> {
        self.as_mutable().into_mut_by_id(component_ids)
    }

    /// Consumes `self` and returns [untyped mutable reference(s)](MutUntyped)
    /// to component(s) with lifetime `'w` for the current entity, based on the
    /// given [`ComponentId`]s.
    ///
    /// **You should prefer to use the typed API [`EntityWorldMut::into_mut`] where
    /// possible and only use this in cases where the actual component types
    /// are not known at compile time.**
    ///
    /// Unlike [`EntityWorldMut::into_mut`], this returns untyped reference(s) to
    /// component(s), and it's the job of the caller to ensure the correct
    /// type(s) are dereferenced (if necessary).
    ///
    /// # Errors
    ///
    /// - Returns [`EntityComponentError::MissingComponent`] if the entity does
    ///   not have a component.
    /// - Returns [`EntityComponentError::AliasedMutability`] if a component
    ///   is requested multiple times.
    ///
    /// # Examples
    ///
    /// For examples on how to use this method, see [`EntityMut::get_mut_by_id`].
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[inline]
    pub fn into_mut_by_id<F: DynamicComponentFetch>(
        self,
        component_ids: F,
    ) -> Result<F::Mut<'w>, EntityComponentError> {
        self.into_mutable().into_mut_by_id(component_ids)
    }

    /// Adds a [`Bundle`] of components to the entity.
    ///
    /// This will overwrite any previous value(s) of the same component type.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[track_caller]
    pub fn insert<T: Bundle>(&mut self, bundle: T) -> &mut Self {
        self.insert_with_caller(
            bundle,
            InsertMode::Replace,
            #[cfg(feature = "track_location")]
            Location::caller(),
        )
    }

    /// Adds a [`Bundle`] of components to the entity without overwriting.
    ///
    /// This will leave any previous value(s) of the same component type
    /// unchanged.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[track_caller]
    pub fn insert_if_new<T: Bundle>(&mut self, bundle: T) -> &mut Self {
        self.insert_with_caller(
            bundle,
            InsertMode::Keep,
            #[cfg(feature = "track_location")]
            Location::caller(),
        )
    }

    /// Split into a new function so we can pass the calling location into the function when using
    /// as a command.
    #[inline]
    pub(crate) fn insert_with_caller<T: Bundle>(
        &mut self,
        bundle: T,
        mode: InsertMode,
        #[cfg(feature = "track_location")] caller: &'static Location,
    ) -> &mut Self {
        self.assert_not_despawned();
        let change_tick = self.world.change_tick();
        let mut bundle_inserter =
            BundleInserter::new::<T>(self.world, self.location.archetype_id, change_tick);
        self.location =
            // SAFETY: location matches current entity. `T` matches `bundle_info`
            unsafe {
                bundle_inserter.insert(self.entity, self.location, bundle, mode, #[cfg(feature = "track_location")] caller)
            };
        self.world.flush();
        self.update_location();
        self
    }

    /// Inserts a dynamic [`Component`] into the entity.
    ///
    /// This will overwrite any previous value(s) of the same component type.
    ///
    /// You should prefer to use the typed API [`EntityWorldMut::insert`] where possible.
    ///
    /// # Safety
    ///
    /// - [`ComponentId`] must be from the same world as [`EntityWorldMut`]
    /// - [`OwningPtr`] must be a valid reference to the type represented by [`ComponentId`]
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[track_caller]
    pub unsafe fn insert_by_id(
        &mut self,
        component_id: ComponentId,
        component: OwningPtr<'_>,
    ) -> &mut Self {
        self.insert_by_id_with_caller(
            component_id,
            component,
            #[cfg(feature = "track_location")]
            Location::caller(),
        )
    }

    /// # Safety
    /// See [`EntityWorldMut::insert_by_id`]
    #[inline]
    pub(crate) unsafe fn insert_by_id_with_caller(
        &mut self,
        component_id: ComponentId,
        component: OwningPtr<'_>,
        #[cfg(feature = "track_location")] caller: &'static Location<'static>,
    ) -> &mut Self {
        self.assert_not_despawned();
        let change_tick = self.world.change_tick();
        let bundle_id = self.world.bundles.init_component_info(
            &mut self.world.storages,
            &self.world.components,
            component_id,
        );
        let storage_type = self.world.bundles.get_storage_unchecked(bundle_id);

        let bundle_inserter = BundleInserter::new_with_id(
            self.world,
            self.location.archetype_id,
            bundle_id,
            change_tick,
        );

        self.location = insert_dynamic_bundle(
            bundle_inserter,
            self.entity,
            self.location,
            Some(component).into_iter(),
            Some(storage_type).iter().cloned(),
            #[cfg(feature = "track_location")]
            caller,
        );
        self.world.flush();
        self.update_location();
        self
    }

    /// Inserts a dynamic [`Bundle`] into the entity.
    ///
    /// This will overwrite any previous value(s) of the same component type.
    ///
    /// You should prefer to use the typed API [`EntityWorldMut::insert`] where possible.
    /// If your [`Bundle`] only has one component, use the cached API [`EntityWorldMut::insert_by_id`].
    ///
    /// If possible, pass a sorted slice of `ComponentId` to maximize caching potential.
    ///
    /// # Safety
    /// - Each [`ComponentId`] must be from the same world as [`EntityWorldMut`]
    /// - Each [`OwningPtr`] must be a valid reference to the type represented by [`ComponentId`]
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[track_caller]
    pub unsafe fn insert_by_ids<'a, I: Iterator<Item = OwningPtr<'a>>>(
        &mut self,
        component_ids: &[ComponentId],
        iter_components: I,
    ) -> &mut Self {
        self.assert_not_despawned();
        let change_tick = self.world.change_tick();
        let bundle_id = self.world.bundles.init_dynamic_info(
            &mut self.world.storages,
            &self.world.components,
            component_ids,
        );
        let mut storage_types =
            core::mem::take(self.world.bundles.get_storages_unchecked(bundle_id));
        let bundle_inserter = BundleInserter::new_with_id(
            self.world,
            self.location.archetype_id,
            bundle_id,
            change_tick,
        );

        self.location = insert_dynamic_bundle(
            bundle_inserter,
            self.entity,
            self.location,
            iter_components,
            (*storage_types).iter().cloned(),
            #[cfg(feature = "track_location")]
            Location::caller(),
        );
        *self.world.bundles.get_storages_unchecked(bundle_id) = core::mem::take(&mut storage_types);
        self.world.flush();
        self.update_location();
        self
    }

    /// Removes all components in the [`Bundle`] from the entity and returns their previous values.
    ///
    /// **Note:** If the entity does not have every component in the bundle, this method will not
    /// remove any of them.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    // TODO: BundleRemover?
    #[must_use]
    #[track_caller]
    pub fn take<T: Bundle>(&mut self) -> Option<T> {
        self.assert_not_despawned();
        let world = &mut self.world;
        let storages = &mut world.storages;
        let components = &mut world.components;
        let bundle_id = world.bundles.register_info::<T>(components, storages);
        // SAFETY: We just ensured this bundle exists
        let bundle_info = unsafe { world.bundles.get_unchecked(bundle_id) };
        let old_location = self.location;
        // SAFETY: `archetype_id` exists because it is referenced in the old `EntityLocation` which is valid,
        // components exist in `bundle_info` because `Bundles::init_info` initializes a `BundleInfo` containing all components of the bundle type `T`
        let new_archetype_id = unsafe {
            bundle_info.remove_bundle_from_archetype(
                &mut world.archetypes,
                storages,
                components,
                &world.observers,
                old_location.archetype_id,
                false,
            )?
        };

        if new_archetype_id == old_location.archetype_id {
            return None;
        }

        let entity = self.entity;
        // SAFETY: Archetypes and Bundles cannot be mutably aliased through DeferredWorld
        let (old_archetype, bundle_info, mut deferred_world) = unsafe {
            let bundle_info: *const BundleInfo = bundle_info;
            let world = world.as_unsafe_world_cell();
            (
                &world.archetypes()[old_location.archetype_id],
                &*bundle_info,
                world.into_deferred(),
            )
        };

        // SAFETY: all bundle components exist in World
        unsafe {
            trigger_on_replace_and_on_remove_hooks_and_observers(
                &mut deferred_world,
                old_archetype,
                entity,
                bundle_info,
                #[cfg(feature = "track_location")]
                Location::caller(),
            );
        }

        let archetypes = &mut world.archetypes;
        let storages = &mut world.storages;
        let components = &mut world.components;
        let entities = &mut world.entities;
        let removed_components = &mut world.removed_components;

        let entity = self.entity;
        let mut bundle_components = bundle_info.iter_explicit_components();
        // SAFETY: bundle components are iterated in order, which guarantees that the component type
        // matches
        let result = unsafe {
            T::from_components(storages, &mut |storages| {
                let component_id = bundle_components.next().unwrap();
                // SAFETY:
                // - entity location is valid
                // - table row is removed below, without dropping the contents
                // - `components` comes from the same world as `storages`
                // - the component exists on the entity
                take_component(
                    storages,
                    components,
                    removed_components,
                    component_id,
                    entity,
                    old_location,
                )
            })
        };

        #[expect(
            clippy::undocumented_unsafe_blocks,
            reason = "Needs to be documented; see #17345."
        )]
        unsafe {
            Self::move_entity_from_remove::<false>(
                entity,
                &mut self.location,
                old_location.archetype_id,
                old_location,
                entities,
                archetypes,
                storages,
                new_archetype_id,
            );
        }
        self.world.flush();
        self.update_location();
        Some(result)
    }

    /// # Safety
    ///
    /// `new_archetype_id` must have the same or a subset of the components
    /// in `old_archetype_id`. Probably more safety stuff too, audit a call to
    /// this fn as if the code here was written inline
    ///
    /// when DROP is true removed components will be dropped otherwise they will be forgotten
    // We use a const generic here so that we are less reliant on
    // inlining for rustc to optimize out the `match DROP`
    unsafe fn move_entity_from_remove<const DROP: bool>(
        entity: Entity,
        self_location: &mut EntityLocation,
        old_archetype_id: ArchetypeId,
        old_location: EntityLocation,
        entities: &mut Entities,
        archetypes: &mut Archetypes,
        storages: &mut Storages,
        new_archetype_id: ArchetypeId,
    ) {
        let old_archetype = &mut archetypes[old_archetype_id];
        let remove_result = old_archetype.swap_remove(old_location.archetype_row);
        // if an entity was moved into this entity's archetype row, update its archetype row
        if let Some(swapped_entity) = remove_result.swapped_entity {
            let swapped_location = entities.get(swapped_entity).unwrap();

            entities.set(
                swapped_entity.index(),
                EntityLocation {
                    archetype_id: swapped_location.archetype_id,
                    archetype_row: old_location.archetype_row,
                    table_id: swapped_location.table_id,
                    table_row: swapped_location.table_row,
                },
            );
        }
        let old_table_row = remove_result.table_row;
        let old_table_id = old_archetype.table_id();
        let new_archetype = &mut archetypes[new_archetype_id];

        let new_location = if old_table_id == new_archetype.table_id() {
            new_archetype.allocate(entity, old_table_row)
        } else {
            let (old_table, new_table) = storages
                .tables
                .get_2_mut(old_table_id, new_archetype.table_id());

            let move_result = if DROP {
                // SAFETY: old_table_row exists
                unsafe { old_table.move_to_and_drop_missing_unchecked(old_table_row, new_table) }
            } else {
                // SAFETY: old_table_row exists
                unsafe { old_table.move_to_and_forget_missing_unchecked(old_table_row, new_table) }
            };

            // SAFETY: move_result.new_row is a valid position in new_archetype's table
            let new_location = unsafe { new_archetype.allocate(entity, move_result.new_row) };

            // if an entity was moved into this entity's table row, update its table row
            if let Some(swapped_entity) = move_result.swapped_entity {
                let swapped_location = entities.get(swapped_entity).unwrap();

                entities.set(
                    swapped_entity.index(),
                    EntityLocation {
                        archetype_id: swapped_location.archetype_id,
                        archetype_row: swapped_location.archetype_row,
                        table_id: swapped_location.table_id,
                        table_row: old_location.table_row,
                    },
                );
                archetypes[swapped_location.archetype_id]
                    .set_entity_table_row(swapped_location.archetype_row, old_table_row);
            }

            new_location
        };

        *self_location = new_location;
        // SAFETY: The entity is valid and has been moved to the new location already.
        unsafe {
            entities.set(entity.index(), new_location);
        }
    }

    /// Remove the components of `bundle` from `entity`.
    ///
    /// # Safety
    /// - A `BundleInfo` with the corresponding `BundleId` must have been initialized.
    unsafe fn remove_bundle(
        &mut self,
        bundle: BundleId,
        #[cfg(feature = "track_location")] caller: &'static Location<'static>,
    ) -> EntityLocation {
        let entity = self.entity;
        let world = &mut self.world;
        let location = self.location;
        // SAFETY: the caller guarantees that the BundleInfo for this id has been initialized.
        let bundle_info = world.bundles.get_unchecked(bundle);

        // SAFETY: `archetype_id` exists because it is referenced in `location` which is valid
        // and components in `bundle_info` must exist due to this function's safety invariants.
        let new_archetype_id = bundle_info
            .remove_bundle_from_archetype(
                &mut world.archetypes,
                &mut world.storages,
                &world.components,
                &world.observers,
                location.archetype_id,
                // components from the bundle that are not present on the entity are ignored
                true,
            )
            .expect("intersections should always return a result");

        if new_archetype_id == location.archetype_id {
            return location;
        }

        // SAFETY: Archetypes and Bundles cannot be mutably aliased through DeferredWorld
        let (old_archetype, bundle_info, mut deferred_world) = unsafe {
            let bundle_info: *const BundleInfo = bundle_info;
            let world = world.as_unsafe_world_cell();
            (
                &world.archetypes()[location.archetype_id],
                &*bundle_info,
                world.into_deferred(),
            )
        };

        // SAFETY: all bundle components exist in World
        unsafe {
            trigger_on_replace_and_on_remove_hooks_and_observers(
                &mut deferred_world,
                old_archetype,
                entity,
                bundle_info,
                #[cfg(feature = "track_location")]
                caller,
            );
        }

        let old_archetype = &world.archetypes[location.archetype_id];
        for component_id in bundle_info.iter_explicit_components() {
            if old_archetype.contains(component_id) {
                world.removed_components.send(component_id, entity);

                // Make sure to drop components stored in sparse sets.
                // Dense components are dropped later in `move_to_and_drop_missing_unchecked`.
                if let Some(StorageType::SparseSet) = old_archetype.get_storage_type(component_id) {
                    world
                        .storages
                        .sparse_sets
                        .get_mut(component_id)
                        // Set exists because the component existed on the entity
                        .unwrap()
                        .remove(entity);
                }
            }
        }

        // SAFETY: `new_archetype_id` is a subset of the components in `old_location.archetype_id`
        // because it is created by removing a bundle from these components.
        let mut new_location = location;
        Self::move_entity_from_remove::<true>(
            entity,
            &mut new_location,
            location.archetype_id,
            location,
            &mut world.entities,
            &mut world.archetypes,
            &mut world.storages,
            new_archetype_id,
        );

        new_location
    }

    /// Removes any components in the [`Bundle`] from the entity.
    ///
    /// See [`EntityCommands::remove`](crate::system::EntityCommands::remove) for more details.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    // TODO: BundleRemover?
    #[track_caller]
    pub fn remove<T: Bundle>(&mut self) -> &mut Self {
        self.remove_with_caller::<T>(
            #[cfg(feature = "track_location")]
            Location::caller(),
        )
    }

    #[inline]
    pub(crate) fn remove_with_caller<T: Bundle>(
        &mut self,
        #[cfg(feature = "track_location")] caller: &'static Location<'static>,
    ) -> &mut Self {
        self.assert_not_despawned();
        let storages = &mut self.world.storages;
        let components = &mut self.world.components;
        let bundle_info = self.world.bundles.register_info::<T>(components, storages);

        // SAFETY: the `BundleInfo` is initialized above
        self.location = unsafe {
            self.remove_bundle(
                bundle_info,
                #[cfg(feature = "track_location")]
                caller,
            )
        };
        self.world.flush();
        self.update_location();
        self
    }

    /// Removes all components in the [`Bundle`] and remove all required components for each component in the bundle
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[track_caller]
    pub fn remove_with_requires<T: Bundle>(&mut self) -> &mut Self {
        self.remove_with_requires_with_caller::<T>(
            #[cfg(feature = "track_location")]
            Location::caller(),
        )
    }

    pub(crate) fn remove_with_requires_with_caller<T: Bundle>(
        &mut self,
        #[cfg(feature = "track_location")] caller: &'static Location<'static>,
    ) -> &mut Self {
        self.assert_not_despawned();
        let storages = &mut self.world.storages;
        let components = &mut self.world.components;
        let bundles = &mut self.world.bundles;

        let bundle_id = bundles.register_contributed_bundle_info::<T>(components, storages);

        // SAFETY: the dynamic `BundleInfo` is initialized above
        self.location = unsafe {
            self.remove_bundle(
                bundle_id,
                #[cfg(feature = "track_location")]
                caller,
            )
        };
        self.world.flush();
        self.update_location();
        self
    }

    /// Removes any components except those in the [`Bundle`] (and its Required Components) from the entity.
    ///
    /// See [`EntityCommands::retain`](crate::system::EntityCommands::retain) for more details.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[track_caller]
    pub fn retain<T: Bundle>(&mut self) -> &mut Self {
        self.retain_with_caller::<T>(
            #[cfg(feature = "track_location")]
            Location::caller(),
        )
    }

    #[inline]
    pub(crate) fn retain_with_caller<T: Bundle>(
        &mut self,
        #[cfg(feature = "track_location")] caller: &'static Location<'static>,
    ) -> &mut Self {
        self.assert_not_despawned();
        let archetypes = &mut self.world.archetypes;
        let storages = &mut self.world.storages;
        let components = &mut self.world.components;

        let retained_bundle = self.world.bundles.register_info::<T>(components, storages);
        // SAFETY: `retained_bundle` exists as we just initialized it.
        let retained_bundle_info = unsafe { self.world.bundles.get_unchecked(retained_bundle) };
        let old_location = self.location;
        let old_archetype = &mut archetypes[old_location.archetype_id];

        // PERF: this could be stored in an Archetype Edge
        let to_remove = &old_archetype
            .components()
            .filter(|c| !retained_bundle_info.contributed_components().contains(c))
            .collect::<Vec<_>>();
        let remove_bundle =
            self.world
                .bundles
                .init_dynamic_info(&mut self.world.storages, components, to_remove);

        // SAFETY: the `BundleInfo` for the components to remove is initialized above
        self.location = unsafe {
            self.remove_bundle(
                remove_bundle,
                #[cfg(feature = "track_location")]
                caller,
            )
        };
        self.world.flush();
        self.update_location();
        self
    }

    /// Removes a dynamic [`Component`] from the entity if it exists.
    ///
    /// You should prefer to use the typed API [`EntityWorldMut::remove`] where possible.
    ///
    /// # Panics
    ///
    /// Panics if the provided [`ComponentId`] does not exist in the [`World`] or if the
    /// entity has been despawned while this `EntityWorldMut` is still alive.
    #[track_caller]
    pub fn remove_by_id(&mut self, component_id: ComponentId) -> &mut Self {
        self.remove_by_id_with_caller(
            component_id,
            #[cfg(feature = "track_location")]
            Location::caller(),
        )
    }

    #[inline]
    pub(crate) fn remove_by_id_with_caller(
        &mut self,
        component_id: ComponentId,
        #[cfg(feature = "track_location")] caller: &'static Location<'static>,
    ) -> &mut Self {
        self.assert_not_despawned();
        let components = &mut self.world.components;

        let bundle_id = self.world.bundles.init_component_info(
            &mut self.world.storages,
            components,
            component_id,
        );

        // SAFETY: the `BundleInfo` for this `component_id` is initialized above
        self.location = unsafe {
            self.remove_bundle(
                bundle_id,
                #[cfg(feature = "track_location")]
                caller,
            )
        };
        self.world.flush();
        self.update_location();
        self
    }

    /// Removes a dynamic bundle from the entity if it exists.
    ///
    /// You should prefer to use the typed API [`EntityWorldMut::remove`] where possible.
    ///
    /// # Panics
    ///
    /// Panics if any of the provided [`ComponentId`]s do not exist in the [`World`] or if the
    /// entity has been despawned while this `EntityWorldMut` is still alive.
    #[track_caller]
    pub fn remove_by_ids(&mut self, component_ids: &[ComponentId]) -> &mut Self {
        self.assert_not_despawned();
        let components = &mut self.world.components;

        let bundle_id = self.world.bundles.init_dynamic_info(
            &mut self.world.storages,
            components,
            component_ids,
        );

        // SAFETY: the `BundleInfo` for this `bundle_id` is initialized above
        unsafe {
            self.remove_bundle(
                bundle_id,
                #[cfg(feature = "track_location")]
                Location::caller(),
            )
        };

        self.world.flush();
        self.update_location();
        self
    }

    /// Removes all components associated with the entity.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[track_caller]
    pub fn clear(&mut self) -> &mut Self {
        self.clear_with_caller(
            #[cfg(feature = "track_location")]
            Location::caller(),
        )
    }

    #[inline]
    pub(crate) fn clear_with_caller(
        &mut self,
        #[cfg(feature = "track_location")] caller: &'static Location<'static>,
    ) -> &mut Self {
        self.assert_not_despawned();
        let component_ids: Vec<ComponentId> = self.archetype().components().collect();
        let components = &mut self.world.components;

        let bundle_id = self.world.bundles.init_dynamic_info(
            &mut self.world.storages,
            components,
            component_ids.as_slice(),
        );

        // SAFETY: the `BundleInfo` for this `component_id` is initialized above
        self.location = unsafe {
            self.remove_bundle(
                bundle_id,
                #[cfg(feature = "track_location")]
                caller,
            )
        };
        self.world.flush();
        self.update_location();
        self
    }

    /// Despawns the current entity.
    ///
    /// See [`World::despawn`] for more details.
    ///
    /// # Note
    ///
    /// This will also despawn any [`Children`](crate::hierarchy::Children) entities, and any other [`RelationshipTarget`](crate::relationship::RelationshipTarget) that is configured
    /// to despawn descendants. This results in "recursive despawn" behavior.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[track_caller]
    pub fn despawn(self) {
        self.despawn_with_caller(
            #[cfg(feature = "track_location")]
            Location::caller(),
        );
    }

    /// Despawns the provided entity and its descendants.
    #[deprecated(
        since = "0.16.0",
        note = "Use entity.despawn(), which now automatically despawns recursively."
    )]
    pub fn despawn_recursive(self) {
        self.despawn();
    }

    pub(crate) fn despawn_with_caller(
        self,
        #[cfg(feature = "track_location")] caller: &'static Location,
    ) {
        self.assert_not_despawned();
        let world = self.world;
        let archetype = &world.archetypes[self.location.archetype_id];

        // SAFETY: Archetype cannot be mutably aliased by DeferredWorld
        let (archetype, mut deferred_world) = unsafe {
            let archetype: *const Archetype = archetype;
            let world = world.as_unsafe_world_cell();
            (&*archetype, world.into_deferred())
        };

        // SAFETY: All components in the archetype exist in world
        unsafe {
            if archetype.has_despawn_observer() {
                deferred_world.trigger_observers(
                    ON_DESPAWN,
                    self.entity,
                    archetype.components(),
                    #[cfg(feature = "track_location")]
                    caller,
                );
            }
            deferred_world.trigger_on_despawn(
                archetype,
                self.entity,
                archetype.components(),
                #[cfg(feature = "track_location")]
                caller,
            );
            if archetype.has_replace_observer() {
                deferred_world.trigger_observers(
                    ON_REPLACE,
                    self.entity,
                    archetype.components(),
                    #[cfg(feature = "track_location")]
                    caller,
                );
            }
            deferred_world.trigger_on_replace(
                archetype,
                self.entity,
                archetype.components(),
                #[cfg(feature = "track_location")]
                caller,
            );
            if archetype.has_remove_observer() {
                deferred_world.trigger_observers(
                    ON_REMOVE,
                    self.entity,
                    archetype.components(),
                    #[cfg(feature = "track_location")]
                    caller,
                );
            }
            deferred_world.trigger_on_remove(
                archetype,
                self.entity,
                archetype.components(),
                #[cfg(feature = "track_location")]
                caller,
            );
        }

        for component_id in archetype.components() {
            world.removed_components.send(component_id, self.entity);
        }

        // Observers and on_remove hooks may reserve new entities, which
        // requires a flush before Entities::free may be called.
        world.flush_entities();

        let location = world
            .entities
            .free(self.entity)
            .expect("entity should exist at this point.");
        let table_row;
        let moved_entity;

        {
            let archetype = &mut world.archetypes[self.location.archetype_id];
            let remove_result = archetype.swap_remove(location.archetype_row);
            if let Some(swapped_entity) = remove_result.swapped_entity {
                let swapped_location = world.entities.get(swapped_entity).unwrap();
                // SAFETY: swapped_entity is valid and the swapped entity's components are
                // moved to the new location immediately after.
                unsafe {
                    world.entities.set(
                        swapped_entity.index(),
                        EntityLocation {
                            archetype_id: swapped_location.archetype_id,
                            archetype_row: location.archetype_row,
                            table_id: swapped_location.table_id,
                            table_row: swapped_location.table_row,
                        },
                    );
                }
            }
            table_row = remove_result.table_row;

            for component_id in archetype.sparse_set_components() {
                // set must have existed for the component to be added.
                let sparse_set = world.storages.sparse_sets.get_mut(component_id).unwrap();
                sparse_set.remove(self.entity);
            }
            // SAFETY: table rows stored in archetypes always exist
            moved_entity = unsafe {
                world.storages.tables[archetype.table_id()].swap_remove_unchecked(table_row)
            };
        };

        if let Some(moved_entity) = moved_entity {
            let moved_location = world.entities.get(moved_entity).unwrap();
            // SAFETY: `moved_entity` is valid and the provided `EntityLocation` accurately reflects
            //         the current location of the entity and its component data.
            unsafe {
                world.entities.set(
                    moved_entity.index(),
                    EntityLocation {
                        archetype_id: moved_location.archetype_id,
                        archetype_row: moved_location.archetype_row,
                        table_id: moved_location.table_id,
                        table_row,
                    },
                );
            }
            world.archetypes[moved_location.archetype_id]
                .set_entity_table_row(moved_location.archetype_row, table_row);
        }
        world.flush();

        #[cfg(feature = "track_location")]
        {
            // SAFETY: No structural changes
            unsafe {
                world
                    .entities_mut()
                    .set_spawned_or_despawned_by(self.entity.index(), caller);
            }
        }
    }

    /// Ensures any commands triggered by the actions of Self are applied, equivalent to [`World::flush`]
    pub fn flush(self) -> Entity {
        self.world.flush();
        self.entity
    }

    /// Gets read-only access to the world that the current entity belongs to.
    #[inline]
    pub fn world(&self) -> &World {
        self.world
    }

    /// Returns this entity's world.
    ///
    /// See [`EntityWorldMut::world_scope`] or [`EntityWorldMut::into_world_mut`] for a safe alternative.
    ///
    /// # Safety
    /// Caller must not modify the world in a way that changes the current entity's location
    /// If the caller _does_ do something that could change the location, `self.update_location()`
    /// must be called before using any other methods on this [`EntityWorldMut`].
    #[inline]
    pub unsafe fn world_mut(&mut self) -> &mut World {
        self.world
    }

    /// Returns this entity's [`World`], consuming itself.
    #[inline]
    pub fn into_world_mut(self) -> &'w mut World {
        self.world
    }

    /// Gives mutable access to this entity's [`World`] in a temporary scope.
    /// This is a safe alternative to using [`EntityWorldMut::world_mut`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use bevy_ecs::prelude::*;
    /// #[derive(Resource, Default, Clone, Copy)]
    /// struct R(u32);
    ///
    /// # let mut world = World::new();
    /// # world.init_resource::<R>();
    /// # let mut entity = world.spawn_empty();
    /// // This closure gives us temporary access to the world.
    /// let new_r = entity.world_scope(|world: &mut World| {
    ///     // Mutate the world while we have access to it.
    ///     let mut r = world.resource_mut::<R>();
    ///     r.0 += 1;
    ///
    ///     // Return a value from the world before giving it back to the `EntityWorldMut`.
    ///     *r
    /// });
    /// # assert_eq!(new_r.0, 1);
    /// ```
    pub fn world_scope<U>(&mut self, f: impl FnOnce(&mut World) -> U) -> U {
        struct Guard<'w, 'a> {
            entity_mut: &'a mut EntityWorldMut<'w>,
        }

        impl Drop for Guard<'_, '_> {
            #[inline]
            fn drop(&mut self) {
                self.entity_mut.update_location();
            }
        }

        // When `guard` is dropped at the end of this scope,
        // it will update the cached `EntityLocation` for this instance.
        // This will run even in case the closure `f` unwinds.
        let guard = Guard { entity_mut: self };
        f(guard.entity_mut.world)
    }

    /// Updates the internal entity location to match the current location in the internal
    /// [`World`].
    ///
    /// This is *only* required when using the unsafe function [`EntityWorldMut::world_mut`],
    /// which enables the location to change.
    pub fn update_location(&mut self) {
        self.location = self
            .world
            .entities()
            .get(self.entity)
            .unwrap_or(EntityLocation::INVALID);
    }

    /// Returns if the entity has been despawned.
    ///
    /// Normally it shouldn't be needed to explicitly check if the entity has been despawned
    /// between commands as this shouldn't happen. However, for some special cases where it
    /// is known that a hook or an observer might despawn the entity while a [`EntityWorldMut`]
    /// reference is still held, this method can be used to check if the entity is still alive
    /// to avoid panicking when calling further methods.
    #[inline]
    pub fn is_despawned(&self) -> bool {
        self.location.archetype_id == ArchetypeId::INVALID
    }

    /// Gets an Entry into the world for this entity and component for in-place manipulation.
    ///
    /// The type parameter specifies which component to get.
    ///
    /// # Examples
    ///
    /// ```
    /// # use bevy_ecs::prelude::*;
    /// #[derive(Component, Default, Clone, Copy, Debug, PartialEq)]
    /// struct Comp(u32);
    ///
    /// # let mut world = World::new();
    /// let mut entity = world.spawn_empty();
    /// entity.entry().or_insert_with(|| Comp(4));
    /// # let entity_id = entity.id();
    /// assert_eq!(world.query::<&Comp>().single(&world).0, 4);
    ///
    /// # let mut entity = world.get_entity_mut(entity_id).unwrap();
    /// entity.entry::<Comp>().and_modify(|mut c| c.0 += 1);
    /// assert_eq!(world.query::<&Comp>().single(&world).0, 5);
    /// ```
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    pub fn entry<'a, T: Component>(&'a mut self) -> Entry<'w, 'a, T> {
        if self.contains::<T>() {
            Entry::Occupied(OccupiedEntry {
                entity_world: self,
                _marker: PhantomData,
            })
        } else {
            Entry::Vacant(VacantEntry {
                entity_world: self,
                _marker: PhantomData,
            })
        }
    }

    /// Triggers the given `event` for this entity, which will run any observers watching for it.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    pub fn trigger(&mut self, event: impl Event) -> &mut Self {
        self.assert_not_despawned();
        self.world.trigger_targets(event, self.entity);
        self.world.flush();
        self.update_location();
        self
    }

    /// Creates an [`Observer`] listening for events of type `E` targeting this entity.
    /// In order to trigger the callback the entity must also match the query when the event is fired.
    ///
    /// # Panics
    ///
    /// If the entity has been despawned while this `EntityWorldMut` is still alive.
    #[track_caller]
    pub fn observe<E: Event, B: Bundle, M>(
        &mut self,
        observer: impl IntoObserverSystem<E, B, M>,
    ) -> &mut Self {
        self.observe_with_caller(
            observer,
            #[cfg(feature = "track_location")]
            Location::caller(),
        )
    }

    pub(crate) fn observe_with_caller<E: Event, B: Bundle, M>(
        &mut self,
        observer: impl IntoObserverSystem<E, B, M>,
        #[cfg(feature = "track_location")] caller: &'static Location<'static>,
    ) -> &mut Self {
        self.assert_not_despawned();
        self.world.spawn_with_caller(
            Observer::new(observer).with_entity(self.entity),
            #[cfg(feature = "track_location")]
            caller,
        );
        self.world.flush();
        self.update_location();
        self
    }

    /// Clones parts of an entity (components, observers, etc.) onto another entity,
    /// configured through [`EntityClonerBuilder`].
    ///
    /// By default, the other entity will receive all the components of the original that implement
    /// [`Clone`] or [`Reflect`](bevy_reflect::Reflect).
    ///
    /// Configure through [`EntityClonerBuilder`] as follows:
    /// ```
    /// # use bevy_ecs::prelude::*;
    /// # #[derive(Component, Clone, PartialEq, Debug)]
    /// # struct ComponentA;
    /// # #[derive(Component, Clone, PartialEq, Debug)]
    /// # struct ComponentB;
    /// # let mut world = World::new();
    /// # let entity = world.spawn((ComponentA, ComponentB)).id();
    /// # let target = world.spawn_empty().id();
    /// world.entity_mut(entity).clone_with(target, |builder| {
    ///     builder.deny::<ComponentB>();
    /// });
    /// # assert_eq!(world.get::<ComponentA>(target), Some(&ComponentA));
    /// # assert_eq!(world.get::<ComponentB>(target), None);
    /// ```
    ///
    /// See [`EntityClonerBuilder`] for more options.
    ///
    /// # Panics
    ///
    /// - If this entity has been despawned while this `EntityWorldMut` is still alive.
    /// - If the target entity does not exist.
    pub fn clone_with(
        &mut self,
        target: Entity,
        config: impl FnOnce(&mut EntityClonerBuilder) + Send + Sync + 'static,
    ) -> &mut Self {
        self.assert_not_despawned();

        let mut builder = EntityCloner::build(self.world);
        config(&mut builder);
        builder.clone_entity(self.entity, target);

        self.world.flush();
        self.update_location();
        self
    }

    /// Spawns a clone of this entity and returns the [`Entity`] of the clone.
    ///
    /// The clone will receive all the components of the original that implement
    /// [`Clone`] or [`Reflect`](bevy_reflect::Reflect).
    ///
    /// To configure cloning behavior (such as only cloning certain components),
    /// use [`EntityWorldMut::clone_and_spawn_with`].
    ///
    /// # Panics
    ///
    /// If this entity has been despawned while this `EntityWorldMut` is still alive.
    pub fn clone_and_spawn(&mut self) -> Entity {
        self.clone_and_spawn_with(|_| {})
    }

    /// Spawns a clone of this entity and allows configuring cloning behavior
    /// using [`EntityClonerBuilder`], returning the [`Entity`] of the clone.
    ///
    /// By default, the clone will receive all the components of the original that implement
    /// [`Clone`] or [`Reflect`](bevy_reflect::Reflect).
    ///
    /// Configure through [`EntityClonerBuilder`] as follows:
    /// ```
    /// # use bevy_ecs::prelude::*;
    /// # #[derive(Component, Clone, PartialEq, Debug)]
    /// # struct ComponentA;
    /// # #[derive(Component, Clone, PartialEq, Debug)]
    /// # struct ComponentB;
    /// # let mut world = World::new();
    /// # let entity = world.spawn((ComponentA, ComponentB)).id();
    /// let entity_clone = world.entity_mut(entity).clone_and_spawn_with(|builder| {
    ///     builder.deny::<ComponentB>();
    /// });
    /// # assert_eq!(world.get::<ComponentA>(entity_clone), Some(&ComponentA));
    /// # assert_eq!(world.get::<ComponentB>(entity_clone), None);
    /// ```
    ///
    /// See [`EntityClonerBuilder`] for more options.
    ///
    /// # Panics
    ///
    /// If this entity has been despawned while this `EntityWorldMut` is still alive.
    pub fn clone_and_spawn_with(
        &mut self,
        config: impl FnOnce(&mut EntityClonerBuilder) + Send + Sync + 'static,
    ) -> Entity {
        self.assert_not_despawned();

        let entity_clone = self.world.entities.reserve_entity();
        self.world.flush();

        let mut builder = EntityCloner::build(self.world);
        config(&mut builder);
        builder.clone_entity(self.entity, entity_clone);

        self.world.flush();
        self.update_location();
        entity_clone
    }

    /// Clones the specified components of this entity and inserts them into another entity.
    ///
    /// Components can only be cloned if they implement
    /// [`Clone`] or [`Reflect`](bevy_reflect::Reflect).
    ///
    /// # Panics
    ///
    /// - If this entity has been despawned while this `EntityWorldMut` is still alive.
    /// - If the target entity does not exist.
    pub fn clone_components<B: Bundle>(&mut self, target: Entity) -> &mut Self {
        self.assert_not_despawned();

        EntityCloner::build(self.world)
            .deny_all()
            .allow::<B>()
            .clone_entity(self.entity, target);

        self.world.flush();
        self.update_location();
        self
    }

    /// Clones the specified components of this entity and inserts them into another entity,
    /// then removes the components from this entity.
    ///
    /// Components can only be cloned if they implement
    /// [`Clone`] or [`Reflect`](bevy_reflect::Reflect).
    ///
    /// # Panics
    ///
    /// - If this entity has been despawned while this `EntityWorldMut` is still alive.
    /// - If the target entity does not exist.
    pub fn move_components<B: Bundle>(&mut self, target: Entity) -> &mut Self {
        self.assert_not_despawned();

        EntityCloner::build(self.world)
            .deny_all()
            .allow::<B>()
            .move_components(true)
            .clone_entity(self.entity, target);

        self.world.flush();
        self.update_location();
        self
    }

    /// Returns the source code location from which this entity has last been spawned.
    #[cfg(feature = "track_location")]
    pub fn spawned_by(&self) -> &'static Location<'static> {
        self.world()
            .entities()
            .entity_get_spawned_or_despawned_by(self.entity)
            .unwrap()
    }
}

/// # Safety
/// All components in the archetype must exist in world
unsafe fn trigger_on_replace_and_on_remove_hooks_and_observers(
    deferred_world: &mut DeferredWorld,
    archetype: &Archetype,
    entity: Entity,
    bundle_info: &BundleInfo,
    #[cfg(feature = "track_location")] caller: &'static Location<'static>,
) {
    if archetype.has_replace_observer() {
        deferred_world.trigger_observers(
            ON_REPLACE,
            entity,
            bundle_info.iter_explicit_components(),
            #[cfg(feature = "track_location")]
            caller,
        );
    }
    deferred_world.trigger_on_replace(
        archetype,
        entity,
        bundle_info.iter_explicit_components(),
        #[cfg(feature = "track_location")]
        caller,
    );
    if archetype.has_remove_observer() {
        deferred_world.trigger_observers(
            ON_REMOVE,
            entity,
            bundle_info.iter_explicit_components(),
            #[cfg(feature = "track_location")]
            caller,
        );
    }
    deferred_world.trigger_on_remove(
        archetype,
        entity,
        bundle_info.iter_explicit_components(),
        #[cfg(feature = "track_location")]
        caller,
    );
}

/// A view into a single entity and component in a world, which may either be vacant or occupied.
///
/// This `enum` can only be constructed from the [`entry`] method on [`EntityWorldMut`].
///
/// [`entry`]: EntityWorldMut::entry
pub enum Entry<'w, 'a, T: Component> {
    /// An occupied entry.
    Occupied(OccupiedEntry<'w, 'a, T>),
    /// A vacant entry.
    Vacant(VacantEntry<'w, 'a, T>),
}

impl<'w, 'a, T: Component<Mutability = Mutable>> Entry<'w, 'a, T> {
    /// Provides in-place mutable access to an occupied entry.
    ///
    /// # Examples
    ///
    /// ```
    /// # use bevy_ecs::prelude::*;
    /// #[derive(Component, Default, Clone, Copy, Debug, PartialEq)]
    /// struct Comp(u32);
    ///
    /// # let mut world = World::new();
    /// let mut entity = world.spawn(Comp(0));
    ///
    /// entity.entry::<Comp>().and_modify(|mut c| c.0 += 1);
    /// assert_eq!(world.query::<&Comp>().single(&world).0, 1);
    /// ```
    #[inline]
    pub fn and_modify<F: FnOnce(Mut<'_, T>)>(self, f: F) -> Self {
        match self {
            Entry::Occupied(mut entry) => {
                f(entry.get_mut());
                Entry::Occupied(entry)
            }
            Entry::Vacant(entry) => Entry::Vacant(entry),
        }
    }
}

impl<'w, 'a, T: Component> Entry<'w, 'a, T> {
    /// Replaces the component of the entry, and returns an [`OccupiedEntry`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use bevy_ecs::prelude::*;
    /// #[derive(Component, Default, Clone, Copy, Debug, PartialEq)]
    /// struct Comp(u32);
    ///
    /// # let mut world = World::new();
    /// let mut entity = world.spawn_empty();
    ///
    /// let entry = entity.entry().insert_entry(Comp(4));
    /// assert_eq!(entry.get(), &Comp(4));
    ///
    /// let entry = entity.entry().insert_entry(Comp(2));
    /// assert_eq!(entry.get(), &Comp(2));
    /// ```
    #[inline]
    pub fn insert_entry(self, component: T) -> OccupiedEntry<'w, 'a, T> {
        match self {
            Entry::Occupied(mut entry) => {
                entry.insert(component);
                entry
            }
            Entry::Vacant(entry) => entry.insert(component),
        }
    }

    /// Ensures the entry has this component by inserting the given default if empty, and
    /// returns a mutable reference to this component in the entry.
    ///
    /// # Examples
    ///
    /// ```
    /// # use bevy_ecs::prelude::*;
    /// #[derive(Component, Default, Clone, Copy, Debug, PartialEq)]
    /// struct Comp(u32);
    ///
    /// # let mut world = World::new();
    /// let mut entity = world.spawn_empty();
    ///
    /// entity.entry().or_insert(Comp(4));
    /// # let entity_id = entity.id();
    /// assert_eq!(world.query::<&Comp>().single(&world).0, 4);
    ///
    /// # let mut entity = world.get_entity_mut(entity_id).unwrap();
    /// entity.entry().or_insert(Comp(15)).into_mut().0 *= 2;
    /// assert_eq!(world.query::<&Comp>().single(&world).0, 8);
    /// ```
    #[inline]
    pub fn or_insert(self, default: T) -> OccupiedEntry<'w, 'a, T> {
        match self {
            Entry::Occupied(entry) => entry,
            Entry::Vacant(entry) => entry.insert(default),
        }
    }

    /// Ensures the entry has this component by inserting the result of the default function if
    /// empty, and returns a mutable reference to this component in the entry.
    ///
    /// # Examples
    ///
    /// ```
    /// # use bevy_ecs::prelude::*;
    /// #[derive(Component, Default, Clone, Copy, Debug, PartialEq)]
    /// struct Comp(u32);
    ///
    /// # let mut world = World::new();
    /// let mut entity = world.spawn_empty();
    ///
    /// entity.entry().or_insert_with(|| Comp(4));
    /// assert_eq!(world.query::<&Comp>().single(&world).0, 4);
    /// ```
    #[inline]
    pub fn or_insert_with<F: FnOnce() -> T>(self, default: F) -> OccupiedEntry<'w, 'a, T> {
        match self {
            Entry::Occupied(entry) => entry,
            Entry::Vacant(entry) => entry.insert(default()),
        }
    }
}

impl<'w, 'a, T: Component + Default> Entry<'w, 'a, T> {
    /// Ensures the entry has this component by inserting the default value if empty, and
    /// returns a mutable reference to this component in the entry.
    ///
    /// # Examples
    ///
    /// ```
    /// # use bevy_ecs::prelude::*;
    /// #[derive(Component, Default, Clone, Copy, Debug, PartialEq)]
    /// struct Comp(u32);
    ///
    /// # let mut world = World::new();
    /// let mut entity = world.spawn_empty();
    ///
    /// entity.entry::<Comp>().or_default();
    /// assert_eq!(world.query::<&Comp>().single(&world).0, 0);
    /// ```
    #[inline]
    pub fn or_default(self) -> OccupiedEntry<'w, 'a, T> {
        match self {
            Entry::Occupied(entry) => entry,
            Entry::Vacant(entry) => entry.insert(Default::default()),
        }
    }
}

/// A view into an occupied entry in a [`EntityWorldMut`]. It is part of the [`Entry`] enum.
///
/// The contained entity must have the component type parameter if we have this struct.
pub struct OccupiedEntry<'w, 'a, T: Component> {
    entity_world: &'a mut EntityWorldMut<'w>,
    _marker: PhantomData<T>,
}

impl<'w, 'a, T: Component> OccupiedEntry<'w, 'a, T> {
    /// Gets a reference to the component in the entry.
    ///
    /// # Examples
    ///
    /// ```
    /// # use bevy_ecs::{prelude::*, world::Entry};
    /// #[derive(Component, Default, Clone, Copy, Debug, PartialEq)]
    /// struct Comp(u32);
    ///
    /// # let mut world = World::new();
    /// let mut entity = world.spawn(Comp(5));
    ///
    /// if let Entry::Occupied(o) = entity.entry::<Comp>() {
    ///     assert_eq!(o.get().0, 5);
    /// }
    /// ```
    #[inline]
    pub fn get(&self) -> &T {
        // This shouldn't panic because if we have an OccupiedEntry the component must exist.
        self.entity_world.get::<T>().unwrap()
    }

    /// Replaces the component of the entry.
    ///
    /// # Examples
    ///
    /// ```
    /// # use bevy_ecs::{prelude::*, world::Entry};
    /// #[derive(Component, Default, Clone, Copy, Debug, PartialEq)]
    /// struct Comp(u32);
    ///
    /// # let mut world = World::new();
    /// let mut entity = world.spawn(Comp(5));
    ///
    /// if let Entry::Occupied(mut o) = entity.entry::<Comp>() {
    ///     o.insert(Comp(10));
    /// }
    ///
    /// assert_eq!(world.query::<&Comp>().single(&world).0, 10);
    /// ```
    #[inline]
    pub fn insert(&mut self, component: T) {
        self.entity_world.insert(component);
    }

    /// Removes the component from the entry and returns it.
    ///
    /// # Examples
    ///
    /// ```
    /// # use bevy_ecs::{prelude::*, world::Entry};
    /// #[derive(Component, Default, Clone, Copy, Debug, PartialEq)]
    /// struct Comp(u32);
    ///
    /// # let mut world = World::new();
    /// let mut entity = world.spawn(Comp(5));
    ///
    /// if let Entry::Occupied(o) = entity.entry::<Comp>() {
    ///     assert_eq!(o.take(), Comp(5));
    /// }
    ///
    /// assert_eq!(world.query::<&Comp>().iter(&world).len(), 0);
    /// ```
    #[inline]
    pub fn take(self) -> T {
        // This shouldn't panic because if we have an OccupiedEntry the component must exist.
        self.entity_world.take().unwrap()
    }
}

impl<'w, 'a, T: Component<Mutability = Mutable>> OccupiedEntry<'w, 'a, T> {
    /// Gets a mutable reference to the component in the entry.
    ///
    /// If you need a reference to the `OccupiedEntry` which may outlive the destruction of
    /// the `Entry` value, see [`into_mut`].
    ///
    /// [`into_mut`]: Self::into_mut
    ///
    /// # Examples
    ///
    /// ```
    /// # use bevy_ecs::{prelude::*, world::Entry};
    /// #[derive(Component, Default, Clone, Copy, Debug, PartialEq)]
    /// struct Comp(u32);
    ///
    /// # let mut world = World::new();
    /// let mut entity = world.spawn(Comp(5));
    ///
    /// if let Entry::Occupied(mut o) = entity.entry::<Comp>() {
    ///     o.get_mut().0 += 10;
    ///     assert_eq!(o.get().0, 15);
    ///
    ///     // We can use the same Entry multiple times.
    ///     o.get_mut().0 += 2
    /// }
    ///
    /// assert_eq!(world.query::<&Comp>().single(&world).0, 17);
    /// ```
    #[inline]
    pub fn get_mut(&mut self) -> Mut<'_, T> {
        // This shouldn't panic because if we have an OccupiedEntry the component must exist.
        self.entity_world.get_mut::<T>().unwrap()
    }

    /// Converts the `OccupiedEntry` into a mutable reference to the value in the entry with
    /// a lifetime bound to the `EntityWorldMut`.
    ///
    /// If you need multiple references to the `OccupiedEntry`, see [`get_mut`].
    ///
    /// [`get_mut`]: Self::get_mut
    ///
    /// # Examples
    ///
    /// ```
    /// # use bevy_ecs::{prelude::*, world::Entry};
    /// #[derive(Component, Default, Clone, Copy, Debug, PartialEq)]
    /// struct Comp(u32);
    ///
    /// # let mut world = World::new();
    /// let mut entity = world.spawn(Comp(5));
    ///
    /// if let Entry::Occupied(o) = entity.entry::<Comp>() {
    ///     o.into_mut().0 += 10;
    /// }
    ///
    /// assert_eq!(world.query::<&Comp>().single(&world).0, 15);
    /// ```
    #[inline]
    pub fn into_mut(self) -> Mut<'a, T> {
        // This shouldn't panic because if we have an OccupiedEntry the component must exist.
        self.entity_world.get_mut().unwrap()
    }
}

/// A view into a vacant entry in a [`EntityWorldMut`]. It is part of the [`Entry`] enum.
pub struct VacantEntry<'w, 'a, T: Component> {
    entity_world: &'a mut EntityWorldMut<'w>,
    _marker: PhantomData<T>,
}

impl<'w, 'a, T: Component> VacantEntry<'w, 'a, T> {
    /// Inserts the component into the `VacantEntry` and returns an `OccupiedEntry`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use bevy_ecs::{prelude::*, world::Entry};
    /// #[derive(Component, Default, Clone, Copy, Debug, PartialEq)]
    /// struct Comp(u32);
    ///
    /// # let mut world = World::new();
    /// let mut entity = world.spawn_empty();
    ///
    /// if let Entry::Vacant(v) = entity.entry::<Comp>() {
    ///     v.insert(Comp(10));
    /// }
    ///
    /// assert_eq!(world.query::<&Comp>().single(&world).0, 10);
    /// ```
    #[inline]
    pub fn insert(self, component: T) -> OccupiedEntry<'w, 'a, T> {
        self.entity_world.insert(component);
        OccupiedEntry {
            entity_world: self.entity_world,
            _marker: PhantomData,
        }
    }
}

/// Provides read-only access to a single entity and some of its components defined by the contained [`Access`].
///
/// To define the access when used as a [`QueryData`](crate::query::QueryData),
/// use a [`QueryBuilder`](crate::query::QueryBuilder) or [`QueryParamBuilder`](crate::system::QueryParamBuilder).
/// The `FilteredEntityRef` must be the entire `QueryData`, and not nested inside a tuple with other data.
///
/// ```
/// # use bevy_ecs::{prelude::*, world::FilteredEntityRef};
/// #
/// # #[derive(Component)]
/// # struct A;
/// #
/// # let mut world = World::new();
/// # world.spawn(A);
/// #
/// // This gives the `FilteredEntityRef` access to `&A`.
/// let mut query = QueryBuilder::<FilteredEntityRef>::new(&mut world)
///     .data::<&A>()
///     .build();
///
/// let filtered_entity: FilteredEntityRef = query.single(&mut world);
/// let component: &A = filtered_entity.get().unwrap();
///
/// // Here `FilteredEntityRef` is nested in a tuple, so it does not have access to `&A`.
/// let mut query = QueryBuilder::<(Entity, FilteredEntityRef)>::new(&mut world)
///     .data::<&A>()
///     .build();
///
/// let (_, filtered_entity) = query.single(&mut world);
/// assert!(filtered_entity.get::<A>().is_none());
/// ```
#[derive(Clone)]
pub struct FilteredEntityRef<'w> {
    entity: UnsafeEntityCell<'w>,
    access: Access<ComponentId>,
}

impl<'w> FilteredEntityRef<'w> {
    /// # Safety
    /// - No `&mut World` can exist from the underlying `UnsafeWorldCell`
    /// - If `access` takes read access to a component no mutable reference to that
    ///     component can exist at the same time as the returned [`FilteredEntityMut`]
    /// - If `access` takes any access for a component `entity` must have that component.
    #[inline]
    pub(crate) unsafe fn new(entity: UnsafeEntityCell<'w>, access: Access<ComponentId>) -> Self {
        Self { entity, access }
    }

    /// Returns the [ID](Entity) of the current entity.
    #[inline]
    #[must_use = "Omit the .id() call if you do not need to store the `Entity` identifier."]
    pub fn id(&self) -> Entity {
        self.entity.id()
    }

    /// Gets metadata indicating the location where the current entity is stored.
    #[inline]
    pub fn location(&self) -> EntityLocation {
        self.entity.location()
    }

    /// Returns the archetype that the current entity belongs to.
    #[inline]
    pub fn archetype(&self) -> &Archetype {
        self.entity.archetype()
    }

    /// Returns a reference to the underlying [`Access`].
    #[inline]
    pub fn access(&self) -> &Access<ComponentId> {
        &self.access
    }

    /// Returns `true` if the current entity has a component of type `T`.
    /// Otherwise, this returns `false`.
    ///
    /// ## Notes
    ///
    /// If you do not know the concrete type of a component, consider using
    /// [`Self::contains_id`] or [`Self::contains_type_id`].
    #[inline]
    pub fn contains<T: Component>(&self) -> bool {
        self.contains_type_id(TypeId::of::<T>())
    }

    /// Returns `true` if the current entity has a component identified by `component_id`.
    /// Otherwise, this returns false.
    ///
    /// ## Notes
    ///
    /// - If you know the concrete type of the component, you should prefer [`Self::contains`].
    /// - If you know the component's [`TypeId`] but not its [`ComponentId`], consider using
    ///     [`Self::contains_type_id`].
    #[inline]
    pub fn contains_id(&self, component_id: ComponentId) -> bool {
        self.entity.contains_id(component_id)
    }

    /// Returns `true` if the current entity has a component with the type identified by `type_id`.
    /// Otherwise, this returns false.
    ///
    /// ## Notes
    ///
    /// - If you know the concrete type of the component, you should prefer [`Self::contains`].
    /// - If you have a [`ComponentId`] instead of a [`TypeId`], consider using [`Self::contains_id`].
    #[inline]
    pub fn contains_type_id(&self, type_id: TypeId) -> bool {
        self.entity.contains_type_id(type_id)
    }

    /// Gets access to the component of type `T` for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    #[inline]
    pub fn get<T: Component>(&self) -> Option<&'w T> {
        let id = self.entity.world().components().get_id(TypeId::of::<T>())?;
        self.access
            .has_component_read(id)
            // SAFETY: We have read access
            .then(|| unsafe { self.entity.get() })
            .flatten()
    }

    /// Gets access to the component of type `T` for the current entity,
    /// including change detection information as a [`Ref`].
    ///
    /// Returns `None` if the entity does not have a component of type `T`.
    #[inline]
    pub fn get_ref<T: Component>(&self) -> Option<Ref<'w, T>> {
        let id = self.entity.world().components().get_id(TypeId::of::<T>())?;
        self.access
            .has_component_read(id)
            // SAFETY: We have read access
            .then(|| unsafe { self.entity.get_ref() })
            .flatten()
    }

    /// Retrieves the change ticks for the given component. This can be useful for implementing change
    /// detection in custom runtimes.
    #[inline]
    pub fn get_change_ticks<T: Component>(&self) -> Option<ComponentTicks> {
        let id = self.entity.world().components().get_id(TypeId::of::<T>())?;
        self.access
            .has_component_read(id)
            // SAFETY: We have read access
            .then(|| unsafe { self.entity.get_change_ticks::<T>() })
            .flatten()
    }

    /// Retrieves the change ticks for the given [`ComponentId`]. This can be useful for implementing change
    /// detection in custom runtimes.
    ///
    /// **You should prefer to use the typed API [`Self::get_change_ticks`] where possible and only
    /// use this in cases where the actual component types are not known at
    /// compile time.**
    #[inline]
    pub fn get_change_ticks_by_id(&self, component_id: ComponentId) -> Option<ComponentTicks> {
        self.access
            .has_component_read(component_id)
            // SAFETY: We have read access
            .then(|| unsafe { self.entity.get_change_ticks_by_id(component_id) })
            .flatten()
    }

    /// Gets the component of the given [`ComponentId`] from the entity.
    ///
    /// **You should prefer to use the typed API [`Self::get`] where possible and only
    /// use this in cases where the actual component types are not known at
    /// compile time.**
    ///
    /// Unlike [`FilteredEntityRef::get`], this returns a raw pointer to the component,
    /// which is only valid while the [`FilteredEntityRef`] is alive.
    #[inline]
    pub fn get_by_id(&self, component_id: ComponentId) -> Option<Ptr<'w>> {
        self.access
            .has_component_read(component_id)
            // SAFETY: We have read access
            .then(|| unsafe { self.entity.get_by_id(component_id) })
            .flatten()
    }

    /// Returns the source code location from which this entity has been spawned.
    #[cfg(feature = "track_location")]
    pub fn spawned_by(&self) -> &'static Location<'static> {
        self.entity.spawned_by()
    }
}

impl<'w> From<FilteredEntityMut<'w>> for FilteredEntityRef<'w> {
    #[inline]
    fn from(entity: FilteredEntityMut<'w>) -> Self {
        // SAFETY:
        // - `FilteredEntityMut` guarantees exclusive access to all components in the new `FilteredEntityRef`.
        unsafe { FilteredEntityRef::new(entity.entity, entity.access) }
    }
}

impl<'a> From<&'a FilteredEntityMut<'_>> for FilteredEntityRef<'a> {
    #[inline]
    fn from(entity: &'a FilteredEntityMut<'_>) -> Self {
        // SAFETY:
        // - `FilteredEntityMut` guarantees exclusive access to all components in the new `FilteredEntityRef`.
        unsafe { FilteredEntityRef::new(entity.entity, entity.access.clone()) }
    }
}

impl<'a> From<EntityRef<'a>> for FilteredEntityRef<'a> {
    fn from(entity: EntityRef<'a>) -> Self {
        // SAFETY:
        // - `EntityRef` guarantees exclusive access to all components in the new `FilteredEntityRef`.
        unsafe {
            let mut access = Access::default();
            access.read_all();
            FilteredEntityRef::new(entity.cell, access)
        }
    }
}

impl<'a> From<&'a EntityRef<'_>> for FilteredEntityRef<'a> {
    fn from(entity: &'a EntityRef<'_>) -> Self {
        // SAFETY:
        // - `EntityRef` guarantees exclusive access to all components in the new `FilteredEntityRef`.
        unsafe {
            let mut access = Access::default();
            access.read_all();
            FilteredEntityRef::new(entity.cell, access)
        }
    }
}

impl<'a> From<EntityMut<'a>> for FilteredEntityRef<'a> {
    fn from(entity: EntityMut<'a>) -> Self {
        // SAFETY:
        // - `EntityMut` guarantees exclusive access to all components in the new `FilteredEntityRef`.
        unsafe {
            let mut access = Access::default();
            access.read_all();
            FilteredEntityRef::new(entity.cell, access)
        }
    }
}

impl<'a> From<&'a EntityMut<'_>> for FilteredEntityRef<'a> {
    fn from(entity: &'a EntityMut<'_>) -> Self {
        // SAFETY:
        // - `EntityMut` guarantees exclusive access to all components in the new `FilteredEntityRef`.
        unsafe {
            let mut access = Access::default();
            access.read_all();
            FilteredEntityRef::new(entity.cell, access)
        }
    }
}

impl<'a> From<EntityWorldMut<'a>> for FilteredEntityRef<'a> {
    fn from(entity: EntityWorldMut<'a>) -> Self {
        // SAFETY:
        // - `EntityWorldMut` guarantees exclusive access to the entire world.
        unsafe {
            let mut access = Access::default();
            access.read_all();
            FilteredEntityRef::new(entity.into_unsafe_entity_cell(), access)
        }
    }
}

impl<'a> From<&'a EntityWorldMut<'_>> for FilteredEntityRef<'a> {
    fn from(entity: &'a EntityWorldMut<'_>) -> Self {
        // SAFETY:
        // - `EntityWorldMut` guarantees exclusive access to the entire world.
        unsafe {
            let mut access = Access::default();
            access.read_all();
            FilteredEntityRef::new(entity.as_unsafe_entity_cell_readonly(), access)
        }
    }
}

impl PartialEq for FilteredEntityRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.entity() == other.entity()
    }
}

impl Eq for FilteredEntityRef<'_> {}

impl PartialOrd for FilteredEntityRef<'_> {
    /// [`FilteredEntityRef`]'s comparison trait implementations match the underlying [`Entity`],
    /// and cannot discern between different worlds.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FilteredEntityRef<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.entity().cmp(&other.entity())
    }
}

impl Hash for FilteredEntityRef<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.entity().hash(state);
    }
}

impl EntityBorrow for FilteredEntityRef<'_> {
    fn entity(&self) -> Entity {
        self.id()
    }
}

// SAFETY: This type represents one Entity. We implement the comparison traits based on that Entity.
unsafe impl TrustedEntityBorrow for FilteredEntityRef<'_> {}

/// Provides mutable access to a single entity and some of its components defined by the contained [`Access`].
///
/// To define the access when used as a [`QueryData`](crate::query::QueryData),
/// use a [`QueryBuilder`](crate::query::QueryBuilder) or [`QueryParamBuilder`](crate::system::QueryParamBuilder).
/// The `FilteredEntityMut` must be the entire `QueryData`, and not nested inside a tuple with other data.
///
/// ```
/// # use bevy_ecs::{prelude::*, world::FilteredEntityMut};
/// #
/// # #[derive(Component)]
/// # struct A;
/// #
/// # let mut world = World::new();
/// # world.spawn(A);
/// #
/// // This gives the `FilteredEntityMut` access to `&mut A`.
/// let mut query = QueryBuilder::<FilteredEntityMut>::new(&mut world)
///     .data::<&mut A>()
///     .build();
///
/// let mut filtered_entity: FilteredEntityMut = query.single_mut(&mut world);
/// let component: Mut<A> = filtered_entity.get_mut().unwrap();
///
/// // Here `FilteredEntityMut` is nested in a tuple, so it does not have access to `&mut A`.
/// let mut query = QueryBuilder::<(Entity, FilteredEntityMut)>::new(&mut world)
///     .data::<&mut A>()
///     .build();
///
/// let (_, mut filtered_entity) = query.single_mut(&mut world);
/// assert!(filtered_entity.get_mut::<A>().is_none());
/// ```
pub struct FilteredEntityMut<'w> {
    entity: UnsafeEntityCell<'w>,
    access: Access<ComponentId>,
}

impl<'w> FilteredEntityMut<'w> {
    /// # Safety
    /// - No `&mut World` can exist from the underlying `UnsafeWorldCell`
    /// - If `access` takes read access to a component no mutable reference to that
    ///     component can exist at the same time as the returned [`FilteredEntityMut`]
    /// - If `access` takes write access to a component, no reference to that component
    ///     may exist at the same time as the returned [`FilteredEntityMut`]
    /// - If `access` takes any access for a component `entity` must have that component.
    #[inline]
    pub(crate) unsafe fn new(entity: UnsafeEntityCell<'w>, access: Access<ComponentId>) -> Self {
        Self { entity, access }
    }

    /// Returns a new instance with a shorter lifetime.
    /// This is useful if you have `&mut FilteredEntityMut`, but you need `FilteredEntityMut`.
    pub fn reborrow(&mut self) -> FilteredEntityMut<'_> {
        // SAFETY: We have exclusive access to the entire entity and its components.
        unsafe { Self::new(self.entity, self.access.clone()) }
    }

    /// Gets read-only access to all of the entity's components.
    #[inline]
    pub fn as_readonly(&self) -> FilteredEntityRef<'_> {
        FilteredEntityRef::from(self)
    }

    /// Returns the [ID](Entity) of the current entity.
    #[inline]
    #[must_use = "Omit the .id() call if you do not need to store the `Entity` identifier."]
    pub fn id(&self) -> Entity {
        self.entity.id()
    }

    /// Gets metadata indicating the location where the current entity is stored.
    #[inline]
    pub fn location(&self) -> EntityLocation {
        self.entity.location()
    }

    /// Returns the archetype that the current entity belongs to.
    #[inline]
    pub fn archetype(&self) -> &Archetype {
        self.entity.archetype()
    }

    /// Returns a reference to the underlying [`Access`].
    #[inline]
    pub fn access(&self) -> &Access<ComponentId> {
        &self.access
    }

    /// Returns `true` if the current entity has a component of type `T`.
    /// Otherwise, this returns `false`.
    ///
    /// ## Notes
    ///
    /// If you do not know the concrete type of a component, consider using
    /// [`Self::contains_id`] or [`Self::contains_type_id`].
    #[inline]
    pub fn contains<T: Component>(&self) -> bool {
        self.contains_type_id(TypeId::of::<T>())
    }

    /// Returns `true` if the current entity has a component identified by `component_id`.
    /// Otherwise, this returns false.
    ///
    /// ## Notes
    ///
    /// - If you know the concrete type of the component, you should prefer [`Self::contains`].
    /// - If you know the component's [`TypeId`] but not its [`ComponentId`], consider using
    ///     [`Self::contains_type_id`].
    #[inline]
    pub fn contains_id(&self, component_id: ComponentId) -> bool {
        self.entity.contains_id(component_id)
    }

    /// Returns `true` if the current entity has a component with the type identified by `type_id`.
    /// Otherwise, this returns false.
    ///
    /// ## Notes
    ///
    /// - If you know the concrete type of the component, you should prefer [`Self::contains`].
    /// - If you have a [`ComponentId`] instead of a [`TypeId`], consider using [`Self::contains_id`].
    #[inline]
    pub fn contains_type_id(&self, type_id: TypeId) -> bool {
        self.entity.contains_type_id(type_id)
    }

    /// Gets access to the component of type `T` for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    #[inline]
    pub fn get<T: Component>(&self) -> Option<&'_ T> {
        self.as_readonly().get()
    }

    /// Gets access to the component of type `T` for the current entity,
    /// including change detection information as a [`Ref`].
    ///
    /// Returns `None` if the entity does not have a component of type `T`.
    #[inline]
    pub fn get_ref<T: Component>(&self) -> Option<Ref<'_, T>> {
        self.as_readonly().get_ref()
    }

    /// Gets mutable access to the component of type `T` for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    #[inline]
    pub fn get_mut<T: Component<Mutability = Mutable>>(&mut self) -> Option<Mut<'_, T>> {
        let id = self.entity.world().components().get_id(TypeId::of::<T>())?;
        self.access
            .has_component_write(id)
            // SAFETY: We have write access
            .then(|| unsafe { self.entity.get_mut() })
            .flatten()
    }

    /// Consumes self and gets mutable access to the component of type `T`
    /// with the world `'w` lifetime for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    #[inline]
    pub fn into_mut<T: Component<Mutability = Mutable>>(self) -> Option<Mut<'w, T>> {
        // SAFETY:
        // - We have write access
        // - The bound `T: Component<Mutability = Mutable>` ensures the component is mutable
        unsafe { self.into_mut_assume_mutable() }
    }

    /// Consumes self and gets mutable access to the component of type `T`
    /// with the world `'w` lifetime for the current entity.
    /// Returns `None` if the entity does not have a component of type `T`.
    ///
    /// # Safety
    ///
    /// - `T` must be a mutable component
    #[inline]
    pub unsafe fn into_mut_assume_mutable<T: Component>(self) -> Option<Mut<'w, T>> {
        let id = self.entity.world().components().get_id(TypeId::of::<T>())?;
        self.access
            .has_component_write(id)
            // SAFETY:
            // - We have write access
            // - Caller ensures `T` is a mutable component
            .then(|| unsafe { self.entity.get_mut_assume_mutable() })
            .flatten()
    }

    /// Retrieves the change ticks for the given component. This can be useful for implementing change
    /// detection in custom runtimes.
    #[inline]
    pub fn get_change_ticks<T: Component>(&self) -> Option<ComponentTicks> {
        self.as_readonly().get_change_ticks::<T>()
    }

    /// Retrieves the change ticks for the given [`ComponentId`]. This can be useful for implementing change
    /// detection in custom runtimes.
    ///
    /// **You should prefer to use the typed API [`Self::get_change_ticks`] where possible and only
    /// use this in cases where the actual component types are not known at
    /// compile time.**
    #[inline]
    pub fn get_change_ticks_by_id(&self, component_id: ComponentId) -> Option<ComponentTicks> {
        self.as_readonly().get_change_ticks_by_id(component_id)
    }

    /// Gets the component of the given [`ComponentId`] from the entity.
    ///
    /// **You should prefer to use the typed API [`Self::get`] where possible and only
    /// use this in cases where the actual component types are not known at
    /// compile time.**
    ///
    /// Unlike [`FilteredEntityMut::get`], this returns a raw pointer to the component,
    /// which is only valid while the [`FilteredEntityMut`] is alive.
    #[inline]
    pub fn get_by_id(&self, component_id: ComponentId) -> Option<Ptr<'_>> {
        self.as_readonly().get_by_id(component_id)
    }

    /// Gets a [`MutUntyped`] of the component of the given [`ComponentId`] from the entity.
    ///
    /// **You should prefer to use the typed API [`Self::get_mut`] where possible and only
    /// use this in cases where the actual component types are not known at
    /// compile time.**
    ///
    /// Unlike [`FilteredEntityMut::get_mut`], this returns a raw pointer to the component,
    /// which is only valid while the [`FilteredEntityMut`] is alive.
    #[inline]
    pub fn get_mut_by_id(&mut self, component_id: ComponentId) -> Option<MutUntyped<'_>> {
        self.access
            .has_component_write(component_id)
            // SAFETY: We have write access
            .then(|| unsafe { self.entity.get_mut_by_id(component_id).ok() })
            .flatten()
    }

    /// Returns the source code location from which this entity has last been spawned.
    #[cfg(feature = "track_location")]
    pub fn spawned_by(&self) -> &'static Location<'static> {
        self.entity.spawned_by()
    }
}

impl<'a> From<EntityMut<'a>> for FilteredEntityMut<'a> {
    fn from(entity: EntityMut<'a>) -> Self {
        // SAFETY:
        // - `EntityMut` guarantees exclusive access to all components in the new `FilteredEntityMut`.
        unsafe {
            let mut access = Access::default();
            access.read_all();
            access.write_all();
            FilteredEntityMut::new(entity.cell, access)
        }
    }
}

impl<'a> From<&'a mut EntityMut<'_>> for FilteredEntityMut<'a> {
    fn from(entity: &'a mut EntityMut<'_>) -> Self {
        // SAFETY:
        // - `EntityMut` guarantees exclusive access to all components in the new `FilteredEntityMut`.
        unsafe {
            let mut access = Access::default();
            access.read_all();
            access.write_all();
            FilteredEntityMut::new(entity.cell, access)
        }
    }
}

impl<'a> From<EntityWorldMut<'a>> for FilteredEntityMut<'a> {
    fn from(entity: EntityWorldMut<'a>) -> Self {
        // SAFETY:
        // - `EntityWorldMut` guarantees exclusive access to the entire world.
        unsafe {
            let mut access = Access::default();
            access.read_all();
            access.write_all();
            FilteredEntityMut::new(entity.into_unsafe_entity_cell(), access)
        }
    }
}

impl<'a> From<&'a mut EntityWorldMut<'_>> for FilteredEntityMut<'a> {
    fn from(entity: &'a mut EntityWorldMut<'_>) -> Self {
        // SAFETY:
        // - `EntityWorldMut` guarantees exclusive access to the entire world.
        unsafe {
            let mut access = Access::default();
            access.read_all();
            access.write_all();
            FilteredEntityMut::new(entity.as_unsafe_entity_cell(), access)
        }
    }
}

impl PartialEq for FilteredEntityMut<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.entity() == other.entity()
    }
}

impl Eq for FilteredEntityMut<'_> {}

impl PartialOrd for FilteredEntityMut<'_> {
    /// [`FilteredEntityMut`]'s comparison trait implementations match the underlying [`Entity`],
    /// and cannot discern between different worlds.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FilteredEntityMut<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.entity().cmp(&other.entity())
    }
}

impl Hash for FilteredEntityMut<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.entity().hash(state);
    }
}

impl EntityBorrow for FilteredEntityMut<'_> {
    fn entity(&self) -> Entity {
        self.id()
    }
}

// SAFETY: This type represents one Entity. We implement the comparison traits based on that Entity.
unsafe impl TrustedEntityBorrow for FilteredEntityMut<'_> {}

/// Error type returned by [`TryFrom`] conversions from filtered entity types
/// ([`FilteredEntityRef`]/[`FilteredEntityMut`]) to full-access entity types
/// ([`EntityRef`]/[`EntityMut`]).
#[derive(Error, Debug)]
pub enum TryFromFilteredError {
    /// Error indicating that the filtered entity does not have read access to
    /// all components.
    #[error("Conversion failed, filtered entity ref does not have read access to all components")]
    MissingReadAllAccess,
    /// Error indicating that the filtered entity does not have write access to
    /// all components.
    #[error("Conversion failed, filtered entity ref does not have write access to all components")]
    MissingWriteAllAccess,
}

/// Provides read-only access to a single entity and all its components, save
/// for an explicitly-enumerated set.
pub struct EntityRefExcept<'w, B>
where
    B: Bundle,
{
    entity: UnsafeEntityCell<'w>,
    phantom: PhantomData<B>,
}

impl<'w, B> EntityRefExcept<'w, B>
where
    B: Bundle,
{
    /// # Safety
    /// Other users of `UnsafeEntityCell` must only have mutable access to the components in `B`.
    pub(crate) unsafe fn new(entity: UnsafeEntityCell<'w>) -> Self {
        Self {
            entity,
            phantom: PhantomData,
        }
    }

    /// Returns the [ID](Entity) of the current entity.
    #[inline]
    #[must_use = "Omit the .id() call if you do not need to store the `Entity` identifier."]
    pub fn id(&self) -> Entity {
        self.entity.id()
    }

    /// Gets access to the component of type `C` for the current entity. Returns
    /// `None` if the component doesn't have a component of that type or if the
    /// type is one of the excluded components.
    #[inline]
    pub fn get<C>(&self) -> Option<&'w C>
    where
        C: Component,
    {
        let components = self.entity.world().components();
        let id = components.component_id::<C>()?;
        if bundle_contains_component::<B>(components, id) {
            None
        } else {
            // SAFETY: We have read access for all components that weren't
            // covered by the `contains` check above.
            unsafe { self.entity.get() }
        }
    }

    /// Gets access to the component of type `C` for the current entity,
    /// including change detection information. Returns `None` if the component
    /// doesn't have a component of that type or if the type is one of the
    /// excluded components.
    #[inline]
    pub fn get_ref<C>(&self) -> Option<Ref<'w, C>>
    where
        C: Component,
    {
        let components = self.entity.world().components();
        let id = components.component_id::<C>()?;
        if bundle_contains_component::<B>(components, id) {
            None
        } else {
            // SAFETY: We have read access for all components that weren't
            // covered by the `contains` check above.
            unsafe { self.entity.get_ref() }
        }
    }

    /// Returns the source code location from which this entity has been spawned.
    #[cfg(feature = "track_location")]
    pub fn spawned_by(&self) -> &'static Location<'static> {
        self.entity.spawned_by()
    }
}

impl<'a, B> From<&'a EntityMutExcept<'_, B>> for EntityRefExcept<'a, B>
where
    B: Bundle,
{
    fn from(entity: &'a EntityMutExcept<'_, B>) -> Self {
        // SAFETY: All accesses that `EntityRefExcept` provides are also
        // accesses that `EntityMutExcept` provides.
        unsafe { EntityRefExcept::new(entity.entity) }
    }
}

impl<B: Bundle> Clone for EntityRefExcept<'_, B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B: Bundle> Copy for EntityRefExcept<'_, B> {}

impl<B: Bundle> PartialEq for EntityRefExcept<'_, B> {
    fn eq(&self, other: &Self) -> bool {
        self.entity() == other.entity()
    }
}

impl<B: Bundle> Eq for EntityRefExcept<'_, B> {}

impl<B: Bundle> PartialOrd for EntityRefExcept<'_, B> {
    /// [`EntityRefExcept`]'s comparison trait implementations match the underlying [`Entity`],
    /// and cannot discern between different worlds.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<B: Bundle> Ord for EntityRefExcept<'_, B> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.entity().cmp(&other.entity())
    }
}

impl<B: Bundle> Hash for EntityRefExcept<'_, B> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.entity().hash(state);
    }
}

impl<B: Bundle> EntityBorrow for EntityRefExcept<'_, B> {
    fn entity(&self) -> Entity {
        self.id()
    }
}

// SAFETY: This type represents one Entity. We implement the comparison traits based on that Entity.
unsafe impl<B: Bundle> TrustedEntityBorrow for EntityRefExcept<'_, B> {}

/// Provides mutable access to all components of an entity, with the exception
/// of an explicit set.
///
/// This is a rather niche type that should only be used if you need access to
/// *all* components of an entity, while still allowing you to consult other
/// queries that might match entities that this query also matches. If you don't
/// need access to all components, prefer a standard query with a
/// [`crate::query::Without`] filter.
pub struct EntityMutExcept<'w, B>
where
    B: Bundle,
{
    entity: UnsafeEntityCell<'w>,
    phantom: PhantomData<B>,
}

impl<'w, B> EntityMutExcept<'w, B>
where
    B: Bundle,
{
    /// # Safety
    /// Other users of `UnsafeEntityCell` must not have access to any components not in `B`.
    pub(crate) unsafe fn new(entity: UnsafeEntityCell<'w>) -> Self {
        Self {
            entity,
            phantom: PhantomData,
        }
    }

    /// Returns the [ID](Entity) of the current entity.
    #[inline]
    #[must_use = "Omit the .id() call if you do not need to store the `Entity` identifier."]
    pub fn id(&self) -> Entity {
        self.entity.id()
    }

    /// Returns a new instance with a shorter lifetime.
    ///
    /// This is useful if you have `&mut EntityMutExcept`, but you need
    /// `EntityMutExcept`.
    pub fn reborrow(&mut self) -> EntityMutExcept<'_, B> {
        // SAFETY: We have exclusive access to the entire entity and the
        // applicable components.
        unsafe { Self::new(self.entity) }
    }

    /// Gets read-only access to all of the entity's components, except for the
    /// ones in `CL`.
    #[inline]
    pub fn as_readonly(&self) -> EntityRefExcept<'_, B> {
        EntityRefExcept::from(self)
    }

    /// Gets access to the component of type `C` for the current entity. Returns
    /// `None` if the component doesn't have a component of that type or if the
    /// type is one of the excluded components.
    #[inline]
    pub fn get<C>(&self) -> Option<&'_ C>
    where
        C: Component,
    {
        self.as_readonly().get()
    }

    /// Gets access to the component of type `C` for the current entity,
    /// including change detection information. Returns `None` if the component
    /// doesn't have a component of that type or if the type is one of the
    /// excluded components.
    #[inline]
    pub fn get_ref<C>(&self) -> Option<Ref<'_, C>>
    where
        C: Component,
    {
        self.as_readonly().get_ref()
    }

    /// Gets mutable access to the component of type `C` for the current entity.
    /// Returns `None` if the component doesn't have a component of that type or
    /// if the type is one of the excluded components.
    #[inline]
    pub fn get_mut<C>(&mut self) -> Option<Mut<'_, C>>
    where
        C: Component<Mutability = Mutable>,
    {
        let components = self.entity.world().components();
        let id = components.component_id::<C>()?;
        if bundle_contains_component::<B>(components, id) {
            None
        } else {
            // SAFETY: We have write access for all components that weren't
            // covered by the `contains` check above.
            unsafe { self.entity.get_mut() }
        }
    }

    /// Returns the source code location from which this entity has been spawned.
    #[cfg(feature = "track_location")]
    pub fn spawned_by(&self) -> &'static Location<'static> {
        self.entity.spawned_by()
    }
}

impl<B: Bundle> PartialEq for EntityMutExcept<'_, B> {
    fn eq(&self, other: &Self) -> bool {
        self.entity() == other.entity()
    }
}

impl<B: Bundle> Eq for EntityMutExcept<'_, B> {}

impl<B: Bundle> PartialOrd for EntityMutExcept<'_, B> {
    /// [`EntityMutExcept`]'s comparison trait implementations match the underlying [`Entity`],
    /// and cannot discern between different worlds.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<B: Bundle> Ord for EntityMutExcept<'_, B> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.entity().cmp(&other.entity())
    }
}

impl<B: Bundle> Hash for EntityMutExcept<'_, B> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.entity().hash(state);
    }
}

impl<B: Bundle> EntityBorrow for EntityMutExcept<'_, B> {
    fn entity(&self) -> Entity {
        self.id()
    }
}

// SAFETY: This type represents one Entity. We implement the comparison traits based on that Entity.
unsafe impl<B: Bundle> TrustedEntityBorrow for EntityMutExcept<'_, B> {}

fn bundle_contains_component<B>(components: &Components, query_id: ComponentId) -> bool
where
    B: Bundle,
{
    let mut found = false;
    B::get_component_ids(components, &mut |maybe_id| {
        if let Some(id) = maybe_id {
            found = found || id == query_id;
        }
    });
    found
}

/// Inserts a dynamic [`Bundle`] into the entity.
///
/// # Safety
///
/// - [`OwningPtr`] and [`StorageType`] iterators must correspond to the
///     [`BundleInfo`] used to construct [`BundleInserter`]
/// - [`Entity`] must correspond to [`EntityLocation`]
unsafe fn insert_dynamic_bundle<
    'a,
    I: Iterator<Item = OwningPtr<'a>>,
    S: Iterator<Item = StorageType>,
>(
    mut bundle_inserter: BundleInserter<'_>,
    entity: Entity,
    location: EntityLocation,
    components: I,
    storage_types: S,
    #[cfg(feature = "track_location")] caller: &'static Location<'static>,
) -> EntityLocation {
    struct DynamicInsertBundle<'a, I: Iterator<Item = (StorageType, OwningPtr<'a>)>> {
        components: I,
    }

    impl<'a, I: Iterator<Item = (StorageType, OwningPtr<'a>)>> DynamicBundle
        for DynamicInsertBundle<'a, I>
    {
        fn get_components(self, func: &mut impl FnMut(StorageType, OwningPtr<'_>)) {
            self.components.for_each(|(t, ptr)| func(t, ptr));
        }
    }

    let bundle = DynamicInsertBundle {
        components: storage_types.zip(components),
    };

    // SAFETY: location matches current entity.
    unsafe {
        bundle_inserter.insert(
            entity,
            location,
            bundle,
            InsertMode::Replace,
            #[cfg(feature = "track_location")]
            caller,
        )
    }
}

/// Moves component data out of storage.
///
/// This function leaves the underlying memory unchanged, but the component behind
/// returned pointer is semantically owned by the caller and will not be dropped in its original location.
/// Caller is responsible to drop component data behind returned pointer.
///
/// # Safety
/// - `location.table_row` must be in bounds of column of component id `component_id`
/// - `component_id` must be valid
/// - `components` must come from the same world as `self`
/// - The relevant table row **must be removed** by the caller once all components are taken, without dropping the value
///
/// # Panics
/// Panics if the entity did not have the component.
#[inline]
pub(crate) unsafe fn take_component<'a>(
    storages: &'a mut Storages,
    components: &Components,
    removed_components: &mut RemovedComponentEvents,
    component_id: ComponentId,
    entity: Entity,
    location: EntityLocation,
) -> OwningPtr<'a> {
    // SAFETY: caller promises component_id to be valid
    let component_info = unsafe { components.get_info_unchecked(component_id) };
    removed_components.send(component_id, entity);
    match component_info.storage_type() {
        StorageType::Table => {
            let table = &mut storages.tables[location.table_id];
            // SAFETY:
            // - archetypes only store valid table_rows
            // - index is in bounds as promised by caller
            // - promote is safe because the caller promises to remove the table row without dropping it immediately afterwards
            unsafe { table.take_component(component_id, location.table_row) }
        }
        StorageType::SparseSet => storages
            .sparse_sets
            .get_mut(component_id)
            .unwrap()
            .remove_and_forget(entity)
            .unwrap(),
    }
}

/// Types that can be used to fetch components from an entity dynamically by
/// [`ComponentId`]s.
///
/// Provided implementations are:
/// - [`ComponentId`]: Returns a single untyped reference.
/// - `[ComponentId; N]` and `&[ComponentId; N]`: Returns a same-sized array of untyped references.
/// - `&[ComponentId]`: Returns a [`Vec`] of untyped references.
/// - [`&HashSet<ComponentId>`](HashSet): Returns a [`HashMap`] of IDs to untyped references.
///
/// # Performance
///
/// - The slice and array implementations perform an aliased mutability check in
///   [`DynamicComponentFetch::fetch_mut`] that is `O(N^2)`.
/// - The [`HashSet`] implementation performs no such check as the type itself
///   guarantees unique IDs.
/// - The single [`ComponentId`] implementation performs no such check as only
///   one reference is returned.
///
/// # Safety
///
/// Implementor must ensure that:
/// - No aliased mutability is caused by the returned references.
/// - [`DynamicComponentFetch::fetch_ref`] returns only read-only references.
pub unsafe trait DynamicComponentFetch {
    /// The read-only reference type returned by [`DynamicComponentFetch::fetch_ref`].
    type Ref<'w>;

    /// The mutable reference type returned by [`DynamicComponentFetch::fetch_mut`].
    type Mut<'w>;

    /// Returns untyped read-only reference(s) to the component(s) with the
    /// given [`ComponentId`]s, as determined by `self`.
    ///
    /// # Safety
    ///
    /// It is the caller's responsibility to ensure that:
    /// - The given [`UnsafeEntityCell`] has read-only access to the fetched components.
    /// - No other mutable references to the fetched components exist at the same time.
    ///
    /// # Errors
    ///
    /// - Returns [`EntityComponentError::MissingComponent`] if a component is missing from the entity.
    unsafe fn fetch_ref(
        self,
        cell: UnsafeEntityCell<'_>,
    ) -> Result<Self::Ref<'_>, EntityComponentError>;

    /// Returns untyped mutable reference(s) to the component(s) with the
    /// given [`ComponentId`]s, as determined by `self`.
    ///
    /// # Safety
    ///
    /// It is the caller's responsibility to ensure that:
    /// - The given [`UnsafeEntityCell`] has mutable access to the fetched components.
    /// - No other references to the fetched components exist at the same time.
    ///
    /// # Errors
    ///
    /// - Returns [`EntityComponentError::MissingComponent`] if a component is missing from the entity.
    /// - Returns [`EntityComponentError::AliasedMutability`] if a component is requested multiple times.
    unsafe fn fetch_mut(
        self,
        cell: UnsafeEntityCell<'_>,
    ) -> Result<Self::Mut<'_>, EntityComponentError>;
}

// SAFETY:
// - No aliased mutability is caused because a single reference is returned.
// - No mutable references are returned by `fetch_ref`.
unsafe impl DynamicComponentFetch for ComponentId {
    type Ref<'w> = Ptr<'w>;
    type Mut<'w> = MutUntyped<'w>;

    unsafe fn fetch_ref(
        self,
        cell: UnsafeEntityCell<'_>,
    ) -> Result<Self::Ref<'_>, EntityComponentError> {
        // SAFETY: caller ensures that the cell has read access to the component.
        unsafe { cell.get_by_id(self) }.ok_or(EntityComponentError::MissingComponent(self))
    }

    unsafe fn fetch_mut(
        self,
        cell: UnsafeEntityCell<'_>,
    ) -> Result<Self::Mut<'_>, EntityComponentError> {
        // SAFETY: caller ensures that the cell has mutable access to the component.
        unsafe { cell.get_mut_by_id(self) }
            .map_err(|_| EntityComponentError::MissingComponent(self))
    }
}

// SAFETY:
// - No aliased mutability is caused because the array is checked for duplicates.
// - No mutable references are returned by `fetch_ref`.
unsafe impl<const N: usize> DynamicComponentFetch for [ComponentId; N] {
    type Ref<'w> = [Ptr<'w>; N];
    type Mut<'w> = [MutUntyped<'w>; N];

    unsafe fn fetch_ref(
        self,
        cell: UnsafeEntityCell<'_>,
    ) -> Result<Self::Ref<'_>, EntityComponentError> {
        <&Self>::fetch_ref(&self, cell)
    }

    unsafe fn fetch_mut(
        self,
        cell: UnsafeEntityCell<'_>,
    ) -> Result<Self::Mut<'_>, EntityComponentError> {
        <&Self>::fetch_mut(&self, cell)
    }
}

// SAFETY:
// - No aliased mutability is caused because the array is checked for duplicates.
// - No mutable references are returned by `fetch_ref`.
unsafe impl<const N: usize> DynamicComponentFetch for &'_ [ComponentId; N] {
    type Ref<'w> = [Ptr<'w>; N];
    type Mut<'w> = [MutUntyped<'w>; N];

    unsafe fn fetch_ref(
        self,
        cell: UnsafeEntityCell<'_>,
    ) -> Result<Self::Ref<'_>, EntityComponentError> {
        let mut ptrs = [const { MaybeUninit::uninit() }; N];
        for (ptr, &id) in core::iter::zip(&mut ptrs, self) {
            *ptr = MaybeUninit::new(
                // SAFETY: caller ensures that the cell has read access to the component.
                unsafe { cell.get_by_id(id) }.ok_or(EntityComponentError::MissingComponent(id))?,
            );
        }

        // SAFETY: Each ptr was initialized in the loop above.
        let ptrs = ptrs.map(|ptr| unsafe { MaybeUninit::assume_init(ptr) });

        Ok(ptrs)
    }

    unsafe fn fetch_mut(
        self,
        cell: UnsafeEntityCell<'_>,
    ) -> Result<Self::Mut<'_>, EntityComponentError> {
        // Check for duplicate component IDs.
        for i in 0..self.len() {
            for j in 0..i {
                if self[i] == self[j] {
                    return Err(EntityComponentError::AliasedMutability(self[i]));
                }
            }
        }

        let mut ptrs = [const { MaybeUninit::uninit() }; N];
        for (ptr, &id) in core::iter::zip(&mut ptrs, self) {
            *ptr = MaybeUninit::new(
                // SAFETY: caller ensures that the cell has mutable access to the component.
                unsafe { cell.get_mut_by_id(id) }
                    .map_err(|_| EntityComponentError::MissingComponent(id))?,
            );
        }

        // SAFETY: Each ptr was initialized in the loop above.
        let ptrs = ptrs.map(|ptr| unsafe { MaybeUninit::assume_init(ptr) });

        Ok(ptrs)
    }
}

// SAFETY:
// - No aliased mutability is caused because the slice is checked for duplicates.
// - No mutable references are returned by `fetch_ref`.
unsafe impl DynamicComponentFetch for &'_ [ComponentId] {
    type Ref<'w> = Vec<Ptr<'w>>;
    type Mut<'w> = Vec<MutUntyped<'w>>;

    unsafe fn fetch_ref(
        self,
        cell: UnsafeEntityCell<'_>,
    ) -> Result<Self::Ref<'_>, EntityComponentError> {
        let mut ptrs = Vec::with_capacity(self.len());
        for &id in self {
            ptrs.push(
                // SAFETY: caller ensures that the cell has read access to the component.
                unsafe { cell.get_by_id(id) }.ok_or(EntityComponentError::MissingComponent(id))?,
            );
        }
        Ok(ptrs)
    }

    unsafe fn fetch_mut(
        self,
        cell: UnsafeEntityCell<'_>,
    ) -> Result<Self::Mut<'_>, EntityComponentError> {
        // Check for duplicate component IDs.
        for i in 0..self.len() {
            for j in 0..i {
                if self[i] == self[j] {
                    return Err(EntityComponentError::AliasedMutability(self[i]));
                }
            }
        }

        let mut ptrs = Vec::with_capacity(self.len());
        for &id in self {
            ptrs.push(
                // SAFETY: caller ensures that the cell has mutable access to the component.
                unsafe { cell.get_mut_by_id(id) }
                    .map_err(|_| EntityComponentError::MissingComponent(id))?,
            );
        }
        Ok(ptrs)
    }
}

// SAFETY:
// - No aliased mutability is caused because `HashSet` guarantees unique elements.
// - No mutable references are returned by `fetch_ref`.
unsafe impl DynamicComponentFetch for &'_ HashSet<ComponentId> {
    type Ref<'w> = HashMap<ComponentId, Ptr<'w>>;
    type Mut<'w> = HashMap<ComponentId, MutUntyped<'w>>;

    unsafe fn fetch_ref(
        self,
        cell: UnsafeEntityCell<'_>,
    ) -> Result<Self::Ref<'_>, EntityComponentError> {
        let mut ptrs = HashMap::with_capacity_and_hasher(self.len(), Default::default());
        for &id in self {
            ptrs.insert(
                id,
                // SAFETY: caller ensures that the cell has read access to the component.
                unsafe { cell.get_by_id(id) }.ok_or(EntityComponentError::MissingComponent(id))?,
            );
        }
        Ok(ptrs)
    }

    unsafe fn fetch_mut(
        self,
        cell: UnsafeEntityCell<'_>,
    ) -> Result<Self::Mut<'_>, EntityComponentError> {
        let mut ptrs = HashMap::with_capacity_and_hasher(self.len(), Default::default());
        for &id in self {
            ptrs.insert(
                id,
                // SAFETY: caller ensures that the cell has mutable access to the component.
                unsafe { cell.get_mut_by_id(id) }
                    .map_err(|_| EntityComponentError::MissingComponent(id))?,
            );
        }
        Ok(ptrs)
    }
}

#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};
    use bevy_ptr::{OwningPtr, Ptr};
    use core::panic::AssertUnwindSafe;

    #[cfg(feature = "track_location")]
    use {core::panic::Location, std::sync::OnceLock};

    use crate::component::HookContext;
    use crate::{
        change_detection::MutUntyped,
        component::ComponentId,
        prelude::*,
        system::{assert_is_system, RunSystemOnce as _},
        world::{error::EntityComponentError, DeferredWorld, FilteredEntityMut, FilteredEntityRef},
    };

    use super::{EntityMutExcept, EntityRefExcept};

    #[derive(Component, Clone, Copy, Debug, PartialEq)]
    struct TestComponent(u32);

    #[derive(Component, Clone, Copy, Debug, PartialEq)]
    #[component(storage = "SparseSet")]
    struct TestComponent2(u32);

    #[test]
    fn entity_ref_get_by_id() {
        let mut world = World::new();
        let entity = world.spawn(TestComponent(42)).id();
        let component_id = world
            .components()
            .get_id(core::any::TypeId::of::<TestComponent>())
            .unwrap();

        let entity = world.entity(entity);
        let test_component = entity.get_by_id(component_id).unwrap();
        // SAFETY: points to a valid `TestComponent`
        let test_component = unsafe { test_component.deref::<TestComponent>() };

        assert_eq!(test_component.0, 42);
    }

    #[test]
    fn entity_mut_get_by_id() {
        let mut world = World::new();
        let entity = world.spawn(TestComponent(42)).id();
        let component_id = world
            .components()
            .get_id(core::any::TypeId::of::<TestComponent>())
            .unwrap();

        let mut entity_mut = world.entity_mut(entity);
        let mut test_component = entity_mut.get_mut_by_id(component_id).unwrap();
        {
            test_component.set_changed();
            let test_component =
                // SAFETY: `test_component` has unique access of the `EntityWorldMut` and is not used afterwards
                unsafe { test_component.into_inner().deref_mut::<TestComponent>() };
            test_component.0 = 43;
        }

        let entity = world.entity(entity);
        let test_component = entity.get_by_id(component_id).unwrap();
        // SAFETY: `TestComponent` is the correct component type
        let test_component = unsafe { test_component.deref::<TestComponent>() };

        assert_eq!(test_component.0, 43);
    }

    #[test]
    fn entity_ref_get_by_id_invalid_component_id() {
        let invalid_component_id = ComponentId::new(usize::MAX);

        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let entity = world.entity(entity);
        assert!(entity.get_by_id(invalid_component_id).is_err());
    }

    #[test]
    fn entity_mut_get_by_id_invalid_component_id() {
        let invalid_component_id = ComponentId::new(usize::MAX);

        let mut world = World::new();
        let mut entity = world.spawn_empty();
        assert!(entity.get_by_id(invalid_component_id).is_err());
        assert!(entity.get_mut_by_id(invalid_component_id).is_err());
    }

    // regression test for https://github.com/bevyengine/bevy/pull/7387
    #[test]
    fn entity_mut_world_scope_panic() {
        let mut world = World::new();

        let mut entity = world.spawn_empty();
        let old_location = entity.location();
        let id = entity.id();
        let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
            entity.world_scope(|w| {
                // Change the entity's `EntityLocation`, which invalidates the original `EntityWorldMut`.
                // This will get updated at the end of the scope.
                w.entity_mut(id).insert(TestComponent(0));

                // Ensure that the entity location still gets updated even in case of a panic.
                panic!("this should get caught by the outer scope")
            });
        }));
        assert!(res.is_err());

        // Ensure that the location has been properly updated.
        assert_ne!(entity.location(), old_location);
    }

    // regression test for https://github.com/bevyengine/bevy/pull/7805
    #[test]
    fn removing_sparse_updates_archetype_row() {
        #[derive(Component, PartialEq, Debug)]
        struct Dense(u8);

        #[derive(Component)]
        #[component(storage = "SparseSet")]
        struct Sparse;

        let mut world = World::new();
        let e1 = world.spawn((Dense(0), Sparse)).id();
        let e2 = world.spawn((Dense(1), Sparse)).id();

        world.entity_mut(e1).remove::<Sparse>();
        assert_eq!(world.entity(e2).get::<Dense>().unwrap(), &Dense(1));
    }

    // regression test for https://github.com/bevyengine/bevy/pull/7805
    #[test]
    fn removing_dense_updates_table_row() {
        #[derive(Component, PartialEq, Debug)]
        struct Dense(u8);

        #[derive(Component)]
        #[component(storage = "SparseSet")]
        struct Sparse;

        let mut world = World::new();
        let e1 = world.spawn((Dense(0), Sparse)).id();
        let e2 = world.spawn((Dense(1), Sparse)).id();

        world.entity_mut(e1).remove::<Dense>();
        assert_eq!(world.entity(e2).get::<Dense>().unwrap(), &Dense(1));
    }

    // Test that calling retain with `()` removes all components.
    #[test]
    fn retain_nothing() {
        #[derive(Component)]
        struct Marker<const N: usize>;

        let mut world = World::new();
        let ent = world.spawn((Marker::<1>, Marker::<2>, Marker::<3>)).id();

        world.entity_mut(ent).retain::<()>();
        assert_eq!(world.entity(ent).archetype().components().next(), None);
    }

    // Test removing some components with `retain`, including components not on the entity.
    #[test]
    fn retain_some_components() {
        #[derive(Component)]
        struct Marker<const N: usize>;

        let mut world = World::new();
        let ent = world.spawn((Marker::<1>, Marker::<2>, Marker::<3>)).id();

        world.entity_mut(ent).retain::<(Marker<2>, Marker<4>)>();
        // Check that marker 2 was retained.
        assert!(world.entity(ent).get::<Marker<2>>().is_some());
        // Check that only marker 2 was retained.
        assert_eq!(
            world
                .entity(ent)
                .archetype()
                .components()
                .collect::<Vec<_>>()
                .len(),
            1
        );
    }

    // regression test for https://github.com/bevyengine/bevy/pull/7805
    #[test]
    fn inserting_sparse_updates_archetype_row() {
        #[derive(Component, PartialEq, Debug)]
        struct Dense(u8);

        #[derive(Component)]
        #[component(storage = "SparseSet")]
        struct Sparse;

        let mut world = World::new();
        let e1 = world.spawn(Dense(0)).id();
        let e2 = world.spawn(Dense(1)).id();

        world.entity_mut(e1).insert(Sparse);
        assert_eq!(world.entity(e2).get::<Dense>().unwrap(), &Dense(1));
    }

    // regression test for https://github.com/bevyengine/bevy/pull/7805
    #[test]
    fn inserting_dense_updates_archetype_row() {
        #[derive(Component, PartialEq, Debug)]
        struct Dense(u8);

        #[derive(Component)]
        struct Dense2;

        #[derive(Component)]
        #[component(storage = "SparseSet")]
        struct Sparse;

        let mut world = World::new();
        let e1 = world.spawn(Dense(0)).id();
        let e2 = world.spawn(Dense(1)).id();

        world.entity_mut(e1).insert(Sparse).remove::<Sparse>();

        // archetype with [e2, e1]
        // table with [e1, e2]

        world.entity_mut(e2).insert(Dense2);

        assert_eq!(world.entity(e1).get::<Dense>().unwrap(), &Dense(0));
    }

    #[test]
    fn inserting_dense_updates_table_row() {
        #[derive(Component, PartialEq, Debug)]
        struct Dense(u8);

        #[derive(Component)]
        struct Dense2;

        #[derive(Component)]
        #[component(storage = "SparseSet")]
        struct Sparse;

        let mut world = World::new();
        let e1 = world.spawn(Dense(0)).id();
        let e2 = world.spawn(Dense(1)).id();

        world.entity_mut(e1).insert(Sparse).remove::<Sparse>();

        // archetype with [e2, e1]
        // table with [e1, e2]

        world.entity_mut(e1).insert(Dense2);

        assert_eq!(world.entity(e2).get::<Dense>().unwrap(), &Dense(1));
    }

    // regression test for https://github.com/bevyengine/bevy/pull/7805
    #[test]
    fn despawning_entity_updates_archetype_row() {
        #[derive(Component, PartialEq, Debug)]
        struct Dense(u8);

        #[derive(Component)]
        #[component(storage = "SparseSet")]
        struct Sparse;

        let mut world = World::new();
        let e1 = world.spawn(Dense(0)).id();
        let e2 = world.spawn(Dense(1)).id();

        world.entity_mut(e1).insert(Sparse).remove::<Sparse>();

        // archetype with [e2, e1]
        // table with [e1, e2]

        world.entity_mut(e2).despawn();

        assert_eq!(world.entity(e1).get::<Dense>().unwrap(), &Dense(0));
    }

    // regression test for https://github.com/bevyengine/bevy/pull/7805
    #[test]
    fn despawning_entity_updates_table_row() {
        #[derive(Component, PartialEq, Debug)]
        struct Dense(u8);

        #[derive(Component)]
        #[component(storage = "SparseSet")]
        struct Sparse;

        let mut world = World::new();
        let e1 = world.spawn(Dense(0)).id();
        let e2 = world.spawn(Dense(1)).id();

        world.entity_mut(e1).insert(Sparse).remove::<Sparse>();

        // archetype with [e2, e1]
        // table with [e1, e2]

        world.entity_mut(e1).despawn();

        assert_eq!(world.entity(e2).get::<Dense>().unwrap(), &Dense(1));
    }

    #[test]
    fn entity_mut_insert_by_id() {
        let mut world = World::new();
        let test_component_id = world.register_component::<TestComponent>();

        let mut entity = world.spawn_empty();
        OwningPtr::make(TestComponent(42), |ptr| {
            // SAFETY: `ptr` matches the component id
            unsafe { entity.insert_by_id(test_component_id, ptr) };
        });

        let components: Vec<_> = world.query::<&TestComponent>().iter(&world).collect();

        assert_eq!(components, vec![&TestComponent(42)]);

        // Compare with `insert_bundle_by_id`

        let mut entity = world.spawn_empty();
        OwningPtr::make(TestComponent(84), |ptr| {
            // SAFETY: `ptr` matches the component id
            unsafe { entity.insert_by_ids(&[test_component_id], vec![ptr].into_iter()) };
        });

        let components: Vec<_> = world.query::<&TestComponent>().iter(&world).collect();

        assert_eq!(components, vec![&TestComponent(42), &TestComponent(84)]);
    }

    #[test]
    fn entity_mut_insert_bundle_by_id() {
        let mut world = World::new();
        let test_component_id = world.register_component::<TestComponent>();
        let test_component_2_id = world.register_component::<TestComponent2>();

        let component_ids = [test_component_id, test_component_2_id];
        let test_component_value = TestComponent(42);
        let test_component_2_value = TestComponent2(84);

        let mut entity = world.spawn_empty();
        OwningPtr::make(test_component_value, |ptr1| {
            OwningPtr::make(test_component_2_value, |ptr2| {
                // SAFETY: `ptr1` and `ptr2` match the component ids
                unsafe { entity.insert_by_ids(&component_ids, vec![ptr1, ptr2].into_iter()) };
            });
        });

        let dynamic_components: Vec<_> = world
            .query::<(&TestComponent, &TestComponent2)>()
            .iter(&world)
            .collect();

        assert_eq!(
            dynamic_components,
            vec![(&TestComponent(42), &TestComponent2(84))]
        );

        // Compare with `World` generated using static type equivalents
        let mut static_world = World::new();

        static_world.spawn((test_component_value, test_component_2_value));
        let static_components: Vec<_> = static_world
            .query::<(&TestComponent, &TestComponent2)>()
            .iter(&static_world)
            .collect();

        assert_eq!(dynamic_components, static_components);
    }

    #[test]
    fn entity_mut_remove_by_id() {
        let mut world = World::new();
        let test_component_id = world.register_component::<TestComponent>();

        let mut entity = world.spawn(TestComponent(42));
        entity.remove_by_id(test_component_id);

        let components: Vec<_> = world.query::<&TestComponent>().iter(&world).collect();

        assert_eq!(components, vec![] as Vec<&TestComponent>);

        // remove non-existent component does not panic
        world.spawn_empty().remove_by_id(test_component_id);
    }

    /// Tests that components can be accessed through an `EntityRefExcept`.
    #[test]
    fn entity_ref_except() {
        let mut world = World::new();
        world.register_component::<TestComponent>();
        world.register_component::<TestComponent2>();

        world.spawn(TestComponent(0)).insert(TestComponent2(0));

        let mut query = world.query::<EntityRefExcept<TestComponent>>();

        let mut found = false;
        for entity_ref in query.iter_mut(&mut world) {
            found = true;
            assert!(entity_ref.get::<TestComponent>().is_none());
            assert!(entity_ref.get_ref::<TestComponent>().is_none());
            assert!(matches!(
                entity_ref.get::<TestComponent2>(),
                Some(TestComponent2(0))
            ));
        }

        assert!(found);
    }

    // Test that a single query can't both contain a mutable reference to a
    // component C and an `EntityRefExcept` that doesn't include C among its
    // exclusions.
    #[test]
    #[should_panic]
    fn entity_ref_except_conflicts_with_self() {
        let mut world = World::new();
        world.spawn(TestComponent(0)).insert(TestComponent2(0));

        // This should panic, because we have a mutable borrow on
        // `TestComponent` but have a simultaneous indirect immutable borrow on
        // that component via `EntityRefExcept`.
        world.run_system_once(system).unwrap();

        fn system(_: Query<(&mut TestComponent, EntityRefExcept<TestComponent2>)>) {}
    }

    // Test that an `EntityRefExcept` that doesn't include a component C among
    // its exclusions can't coexist with a mutable query for that component.
    #[test]
    #[should_panic]
    fn entity_ref_except_conflicts_with_other() {
        let mut world = World::new();
        world.spawn(TestComponent(0)).insert(TestComponent2(0));

        // This should panic, because we have a mutable borrow on
        // `TestComponent` but have a simultaneous indirect immutable borrow on
        // that component via `EntityRefExcept`.
        world.run_system_once(system).unwrap();

        fn system(_: Query<&mut TestComponent>, _: Query<EntityRefExcept<TestComponent2>>) {}
    }

    // Test that an `EntityRefExcept` with an exception for some component C can
    // coexist with a query for that component C.
    #[test]
    fn entity_ref_except_doesnt_conflict() {
        let mut world = World::new();
        world.spawn(TestComponent(0)).insert(TestComponent2(0));

        world.run_system_once(system).unwrap();

        fn system(_: Query<&mut TestComponent>, query: Query<EntityRefExcept<TestComponent>>) {
            for entity_ref in query.iter() {
                assert!(matches!(
                    entity_ref.get::<TestComponent2>(),
                    Some(TestComponent2(0))
                ));
            }
        }
    }

    /// Tests that components can be mutably accessed through an
    /// `EntityMutExcept`.
    #[test]
    fn entity_mut_except() {
        let mut world = World::new();
        world.spawn(TestComponent(0)).insert(TestComponent2(0));

        let mut query = world.query::<EntityMutExcept<TestComponent>>();

        let mut found = false;
        for mut entity_mut in query.iter_mut(&mut world) {
            found = true;
            assert!(entity_mut.get::<TestComponent>().is_none());
            assert!(entity_mut.get_ref::<TestComponent>().is_none());
            assert!(entity_mut.get_mut::<TestComponent>().is_none());
            assert!(matches!(
                entity_mut.get::<TestComponent2>(),
                Some(TestComponent2(0))
            ));
        }

        assert!(found);
    }

    // Test that a single query can't both contain a mutable reference to a
    // component C and an `EntityMutExcept` that doesn't include C among its
    // exclusions.
    #[test]
    #[should_panic]
    fn entity_mut_except_conflicts_with_self() {
        let mut world = World::new();
        world.spawn(TestComponent(0)).insert(TestComponent2(0));

        // This should panic, because we have a mutable borrow on
        // `TestComponent` but have a simultaneous indirect immutable borrow on
        // that component via `EntityRefExcept`.
        world.run_system_once(system).unwrap();

        fn system(_: Query<(&mut TestComponent, EntityMutExcept<TestComponent2>)>) {}
    }

    // Test that an `EntityMutExcept` that doesn't include a component C among
    // its exclusions can't coexist with a query for that component.
    #[test]
    #[should_panic]
    fn entity_mut_except_conflicts_with_other() {
        let mut world = World::new();
        world.spawn(TestComponent(0)).insert(TestComponent2(0));

        // This should panic, because we have a mutable borrow on
        // `TestComponent` but have a simultaneous indirect immutable borrow on
        // that component via `EntityRefExcept`.
        world.run_system_once(system).unwrap();

        fn system(_: Query<&mut TestComponent>, mut query: Query<EntityMutExcept<TestComponent2>>) {
            for mut entity_mut in query.iter_mut() {
                assert!(entity_mut
                    .get_mut::<TestComponent2>()
                    .is_some_and(|component| component.0 == 0));
            }
        }
    }

    // Test that an `EntityMutExcept` with an exception for some component C can
    // coexist with a query for that component C.
    #[test]
    fn entity_mut_except_doesnt_conflict() {
        let mut world = World::new();
        world.spawn(TestComponent(0)).insert(TestComponent2(0));

        world.run_system_once(system).unwrap();

        fn system(_: Query<&mut TestComponent>, mut query: Query<EntityMutExcept<TestComponent>>) {
            for mut entity_mut in query.iter_mut() {
                assert!(entity_mut
                    .get_mut::<TestComponent2>()
                    .is_some_and(|component| component.0 == 0));
            }
        }
    }

    #[derive(Component)]
    struct A;

    #[derive(Resource)]
    struct R;

    #[test]
    fn disjoint_access() {
        fn disjoint_readonly(_: Query<EntityMut, With<A>>, _: Query<EntityRef, Without<A>>) {}

        fn disjoint_mutable(_: Query<EntityMut, With<A>>, _: Query<EntityMut, Without<A>>) {}

        assert_is_system(disjoint_readonly);
        assert_is_system(disjoint_mutable);
    }

    #[test]
    fn ref_compatible() {
        fn borrow_system(_: Query<(EntityRef, &A)>, _: Query<&A>) {}

        assert_is_system(borrow_system);
    }

    #[test]
    fn ref_compatible_with_resource() {
        fn borrow_system(_: Query<EntityRef>, _: Res<R>) {}

        assert_is_system(borrow_system);
    }

    #[test]
    fn ref_compatible_with_resource_mut() {
        fn borrow_system(_: Query<EntityRef>, _: ResMut<R>) {}

        assert_is_system(borrow_system);
    }

    #[test]
    #[should_panic]
    fn ref_incompatible_with_mutable_component() {
        fn incompatible_system(_: Query<(EntityRef, &mut A)>) {}

        assert_is_system(incompatible_system);
    }

    #[test]
    #[should_panic]
    fn ref_incompatible_with_mutable_query() {
        fn incompatible_system(_: Query<EntityRef>, _: Query<&mut A>) {}

        assert_is_system(incompatible_system);
    }

    #[test]
    fn mut_compatible_with_entity() {
        fn borrow_mut_system(_: Query<(Entity, EntityMut)>) {}

        assert_is_system(borrow_mut_system);
    }

    #[test]
    fn mut_compatible_with_resource() {
        fn borrow_mut_system(_: Res<R>, _: Query<EntityMut>) {}

        assert_is_system(borrow_mut_system);
    }

    #[test]
    fn mut_compatible_with_resource_mut() {
        fn borrow_mut_system(_: ResMut<R>, _: Query<EntityMut>) {}

        assert_is_system(borrow_mut_system);
    }

    #[test]
    #[should_panic]
    fn mut_incompatible_with_read_only_component() {
        fn incompatible_system(_: Query<(EntityMut, &A)>) {}

        assert_is_system(incompatible_system);
    }

    #[test]
    #[should_panic]
    fn mut_incompatible_with_mutable_component() {
        fn incompatible_system(_: Query<(EntityMut, &mut A)>) {}

        assert_is_system(incompatible_system);
    }

    #[test]
    #[should_panic]
    fn mut_incompatible_with_read_only_query() {
        fn incompatible_system(_: Query<EntityMut>, _: Query<&A>) {}

        assert_is_system(incompatible_system);
    }

    #[test]
    #[should_panic]
    fn mut_incompatible_with_mutable_query() {
        fn incompatible_system(_: Query<EntityMut>, _: Query<&mut A>) {}

        assert_is_system(incompatible_system);
    }

    #[test]
    fn filtered_entity_ref_normal() {
        let mut world = World::new();
        let a_id = world.register_component::<A>();

        let e: FilteredEntityRef = world.spawn(A).into();

        assert!(e.get::<A>().is_some());
        assert!(e.get_ref::<A>().is_some());
        assert!(e.get_change_ticks::<A>().is_some());
        assert!(e.get_by_id(a_id).is_some());
        assert!(e.get_change_ticks_by_id(a_id).is_some());
    }

    #[test]
    fn filtered_entity_ref_missing() {
        let mut world = World::new();
        let a_id = world.register_component::<A>();

        let e: FilteredEntityRef = world.spawn(()).into();

        assert!(e.get::<A>().is_none());
        assert!(e.get_ref::<A>().is_none());
        assert!(e.get_change_ticks::<A>().is_none());
        assert!(e.get_by_id(a_id).is_none());
        assert!(e.get_change_ticks_by_id(a_id).is_none());
    }

    #[test]
    fn filtered_entity_mut_normal() {
        let mut world = World::new();
        let a_id = world.register_component::<A>();

        let mut e: FilteredEntityMut = world.spawn(A).into();

        assert!(e.get::<A>().is_some());
        assert!(e.get_ref::<A>().is_some());
        assert!(e.get_mut::<A>().is_some());
        assert!(e.get_change_ticks::<A>().is_some());
        assert!(e.get_by_id(a_id).is_some());
        assert!(e.get_mut_by_id(a_id).is_some());
        assert!(e.get_change_ticks_by_id(a_id).is_some());
    }

    #[test]
    fn filtered_entity_mut_missing() {
        let mut world = World::new();
        let a_id = world.register_component::<A>();

        let mut e: FilteredEntityMut = world.spawn(()).into();

        assert!(e.get::<A>().is_none());
        assert!(e.get_ref::<A>().is_none());
        assert!(e.get_mut::<A>().is_none());
        assert!(e.get_change_ticks::<A>().is_none());
        assert!(e.get_by_id(a_id).is_none());
        assert!(e.get_mut_by_id(a_id).is_none());
        assert!(e.get_change_ticks_by_id(a_id).is_none());
    }

    #[derive(Component, PartialEq, Eq, Debug)]
    struct X(usize);

    #[derive(Component, PartialEq, Eq, Debug)]
    struct Y(usize);

    #[test]
    fn get_components() {
        let mut world = World::default();
        let e1 = world.spawn((X(7), Y(10))).id();
        let e2 = world.spawn(X(8)).id();
        let e3 = world.spawn_empty().id();

        assert_eq!(
            Some((&X(7), &Y(10))),
            world.entity(e1).get_components::<(&X, &Y)>()
        );
        assert_eq!(None, world.entity(e2).get_components::<(&X, &Y)>());
        assert_eq!(None, world.entity(e3).get_components::<(&X, &Y)>());
    }

    #[test]
    fn get_by_id_array() {
        let mut world = World::default();
        let e1 = world.spawn((X(7), Y(10))).id();
        let e2 = world.spawn(X(8)).id();
        let e3 = world.spawn_empty().id();

        let x_id = world.register_component::<X>();
        let y_id = world.register_component::<Y>();

        assert_eq!(
            Ok((&X(7), &Y(10))),
            world
                .entity(e1)
                .get_by_id([x_id, y_id])
                .map(|[x_ptr, y_ptr]| {
                    // SAFETY: components match the id they were fetched with
                    (unsafe { x_ptr.deref::<X>() }, unsafe { y_ptr.deref::<Y>() })
                })
        );
        assert_eq!(
            Err(EntityComponentError::MissingComponent(y_id)),
            world
                .entity(e2)
                .get_by_id([x_id, y_id])
                .map(|[x_ptr, y_ptr]| {
                    // SAFETY: components match the id they were fetched with
                    (unsafe { x_ptr.deref::<X>() }, unsafe { y_ptr.deref::<Y>() })
                })
        );
        assert_eq!(
            Err(EntityComponentError::MissingComponent(x_id)),
            world
                .entity(e3)
                .get_by_id([x_id, y_id])
                .map(|[x_ptr, y_ptr]| {
                    // SAFETY: components match the id they were fetched with
                    (unsafe { x_ptr.deref::<X>() }, unsafe { y_ptr.deref::<Y>() })
                })
        );
    }

    #[test]
    fn get_by_id_vec() {
        let mut world = World::default();
        let e1 = world.spawn((X(7), Y(10))).id();
        let e2 = world.spawn(X(8)).id();
        let e3 = world.spawn_empty().id();

        let x_id = world.register_component::<X>();
        let y_id = world.register_component::<Y>();

        assert_eq!(
            Ok((&X(7), &Y(10))),
            world
                .entity(e1)
                .get_by_id(&[x_id, y_id] as &[ComponentId])
                .map(|ptrs| {
                    let Ok([x_ptr, y_ptr]): Result<[Ptr; 2], _> = ptrs.try_into() else {
                        panic!("get_by_id(slice) didn't return 2 elements")
                    };

                    // SAFETY: components match the id they were fetched with
                    (unsafe { x_ptr.deref::<X>() }, unsafe { y_ptr.deref::<Y>() })
                })
        );
        assert_eq!(
            Err(EntityComponentError::MissingComponent(y_id)),
            world
                .entity(e2)
                .get_by_id(&[x_id, y_id] as &[ComponentId])
                .map(|ptrs| {
                    let Ok([x_ptr, y_ptr]): Result<[Ptr; 2], _> = ptrs.try_into() else {
                        panic!("get_by_id(slice) didn't return 2 elements")
                    };

                    // SAFETY: components match the id they were fetched with
                    (unsafe { x_ptr.deref::<X>() }, unsafe { y_ptr.deref::<Y>() })
                })
        );
        assert_eq!(
            Err(EntityComponentError::MissingComponent(x_id)),
            world
                .entity(e3)
                .get_by_id(&[x_id, y_id] as &[ComponentId])
                .map(|ptrs| {
                    let Ok([x_ptr, y_ptr]): Result<[Ptr; 2], _> = ptrs.try_into() else {
                        panic!("get_by_id(slice) didn't return 2 elements")
                    };

                    // SAFETY: components match the id they were fetched with
                    (unsafe { x_ptr.deref::<X>() }, unsafe { y_ptr.deref::<Y>() })
                })
        );
    }

    #[test]
    fn get_mut_by_id_array() {
        let mut world = World::default();
        let e1 = world.spawn((X(7), Y(10))).id();
        let e2 = world.spawn(X(8)).id();
        let e3 = world.spawn_empty().id();

        let x_id = world.register_component::<X>();
        let y_id = world.register_component::<Y>();

        assert_eq!(
            Ok((&mut X(7), &mut Y(10))),
            world
                .entity_mut(e1)
                .get_mut_by_id([x_id, y_id])
                .map(|[x_ptr, y_ptr]| {
                    // SAFETY: components match the id they were fetched with
                    (unsafe { x_ptr.into_inner().deref_mut::<X>() }, unsafe {
                        y_ptr.into_inner().deref_mut::<Y>()
                    })
                })
        );
        assert_eq!(
            Err(EntityComponentError::MissingComponent(y_id)),
            world
                .entity_mut(e2)
                .get_mut_by_id([x_id, y_id])
                .map(|[x_ptr, y_ptr]| {
                    // SAFETY: components match the id they were fetched with
                    (unsafe { x_ptr.into_inner().deref_mut::<X>() }, unsafe {
                        y_ptr.into_inner().deref_mut::<Y>()
                    })
                })
        );
        assert_eq!(
            Err(EntityComponentError::MissingComponent(x_id)),
            world
                .entity_mut(e3)
                .get_mut_by_id([x_id, y_id])
                .map(|[x_ptr, y_ptr]| {
                    // SAFETY: components match the id they were fetched with
                    (unsafe { x_ptr.into_inner().deref_mut::<X>() }, unsafe {
                        y_ptr.into_inner().deref_mut::<Y>()
                    })
                })
        );

        assert_eq!(
            Err(EntityComponentError::AliasedMutability(x_id)),
            world
                .entity_mut(e1)
                .get_mut_by_id([x_id, x_id])
                .map(|_| { unreachable!() })
        );
        assert_eq!(
            Err(EntityComponentError::AliasedMutability(x_id)),
            world
                .entity_mut(e3)
                .get_mut_by_id([x_id, x_id])
                .map(|_| { unreachable!() })
        );
    }

    #[test]
    fn get_mut_by_id_vec() {
        let mut world = World::default();
        let e1 = world.spawn((X(7), Y(10))).id();
        let e2 = world.spawn(X(8)).id();
        let e3 = world.spawn_empty().id();

        let x_id = world.register_component::<X>();
        let y_id = world.register_component::<Y>();

        assert_eq!(
            Ok((&mut X(7), &mut Y(10))),
            world
                .entity_mut(e1)
                .get_mut_by_id(&[x_id, y_id] as &[ComponentId])
                .map(|ptrs| {
                    let Ok([x_ptr, y_ptr]): Result<[MutUntyped; 2], _> = ptrs.try_into() else {
                        panic!("get_mut_by_id(slice) didn't return 2 elements")
                    };

                    // SAFETY: components match the id they were fetched with
                    (unsafe { x_ptr.into_inner().deref_mut::<X>() }, unsafe {
                        y_ptr.into_inner().deref_mut::<Y>()
                    })
                })
        );
        assert_eq!(
            Err(EntityComponentError::MissingComponent(y_id)),
            world
                .entity_mut(e2)
                .get_mut_by_id(&[x_id, y_id] as &[ComponentId])
                .map(|ptrs| {
                    let Ok([x_ptr, y_ptr]): Result<[MutUntyped; 2], _> = ptrs.try_into() else {
                        panic!("get_mut_by_id(slice) didn't return 2 elements")
                    };

                    // SAFETY: components match the id they were fetched with
                    (unsafe { x_ptr.into_inner().deref_mut::<X>() }, unsafe {
                        y_ptr.into_inner().deref_mut::<Y>()
                    })
                })
        );
        assert_eq!(
            Err(EntityComponentError::MissingComponent(x_id)),
            world
                .entity_mut(e3)
                .get_mut_by_id(&[x_id, y_id] as &[ComponentId])
                .map(|ptrs| {
                    let Ok([x_ptr, y_ptr]): Result<[MutUntyped; 2], _> = ptrs.try_into() else {
                        panic!("get_mut_by_id(slice) didn't return 2 elements")
                    };

                    // SAFETY: components match the id they were fetched with
                    (unsafe { x_ptr.into_inner().deref_mut::<X>() }, unsafe {
                        y_ptr.into_inner().deref_mut::<Y>()
                    })
                })
        );

        assert_eq!(
            Err(EntityComponentError::AliasedMutability(x_id)),
            world
                .entity_mut(e1)
                .get_mut_by_id(&[x_id, x_id])
                .map(|_| { unreachable!() })
        );
        assert_eq!(
            Err(EntityComponentError::AliasedMutability(x_id)),
            world
                .entity_mut(e3)
                .get_mut_by_id(&[x_id, x_id])
                .map(|_| { unreachable!() })
        );
    }

    #[test]
    fn get_mut_by_id_unchecked() {
        let mut world = World::default();
        let e1 = world.spawn((X(7), Y(10))).id();
        let x_id = world.register_component::<X>();
        let y_id = world.register_component::<Y>();

        let e1_mut = &world.get_entity_mut([e1]).unwrap()[0];
        // SAFETY: The entity e1 contains component X.
        let x_ptr = unsafe { e1_mut.get_mut_by_id_unchecked(x_id) }.unwrap();
        // SAFETY: The entity e1 contains component Y, with components X and Y being mutually independent.
        let y_ptr = unsafe { e1_mut.get_mut_by_id_unchecked(y_id) }.unwrap();

        // SAFETY: components match the id they were fetched with
        let x_component = unsafe { x_ptr.into_inner().deref_mut::<X>() };
        x_component.0 += 1;
        // SAFETY: components match the id they were fetched with
        let y_component = unsafe { y_ptr.into_inner().deref_mut::<Y>() };
        y_component.0 -= 1;

        assert_eq!((&mut X(8), &mut Y(9)), (x_component, y_component));
    }

    #[derive(Event)]
    struct TestEvent;

    #[test]
    fn adding_observer_updates_location() {
        let mut world = World::new();
        let entity = world
            .spawn_empty()
            .observe(|trigger: Trigger<TestEvent>, mut commands: Commands| {
                commands.entity(trigger.target()).insert(TestComponent(0));
            })
            .id();

        // this should not be needed, but is currently required to tease out the bug
        world.flush();

        let mut a = world.entity_mut(entity);
        a.trigger(TestEvent); // this adds command to change entity archetype
        a.observe(|_: Trigger<TestEvent>| {}); // this flushes commands implicitly by spawning
        let location = a.location();
        assert_eq!(world.entities().get(entity), Some(location));
    }

    #[test]
    #[should_panic]
    fn location_on_despawned_entity_panics() {
        let mut world = World::new();
        world.add_observer(
            |trigger: Trigger<OnAdd, TestComponent>, mut commands: Commands| {
                commands.entity(trigger.target()).despawn();
            },
        );
        let entity = world.spawn_empty().id();
        let mut a = world.entity_mut(entity);
        a.insert(TestComponent(0));
        a.location();
    }

    #[derive(Resource)]
    struct TestFlush(usize);

    fn count_flush(world: &mut World) {
        world.resource_mut::<TestFlush>().0 += 1;
    }

    #[test]
    fn archetype_modifications_trigger_flush() {
        let mut world = World::new();
        world.insert_resource(TestFlush(0));
        world.add_observer(|_: Trigger<OnAdd, TestComponent>, mut commands: Commands| {
            commands.queue(count_flush);
        });
        world.add_observer(
            |_: Trigger<OnRemove, TestComponent>, mut commands: Commands| {
                commands.queue(count_flush);
            },
        );
        world.commands().queue(count_flush);
        let entity = world.spawn_empty().id();
        assert_eq!(world.resource::<TestFlush>().0, 1);
        world.commands().queue(count_flush);
        let mut a = world.entity_mut(entity);
        a.trigger(TestEvent);
        assert_eq!(a.world().resource::<TestFlush>().0, 2);
        a.insert(TestComponent(0));
        assert_eq!(a.world().resource::<TestFlush>().0, 3);
        a.remove::<TestComponent>();
        assert_eq!(a.world().resource::<TestFlush>().0, 4);
        a.insert(TestComponent(0));
        assert_eq!(a.world().resource::<TestFlush>().0, 5);
        let _ = a.take::<TestComponent>();
        assert_eq!(a.world().resource::<TestFlush>().0, 6);
        a.insert(TestComponent(0));
        assert_eq!(a.world().resource::<TestFlush>().0, 7);
        a.retain::<()>();
        assert_eq!(a.world().resource::<TestFlush>().0, 8);
        a.insert(TestComponent(0));
        assert_eq!(a.world().resource::<TestFlush>().0, 9);
        a.clear();
        assert_eq!(a.world().resource::<TestFlush>().0, 10);
        a.insert(TestComponent(0));
        assert_eq!(a.world().resource::<TestFlush>().0, 11);
        a.despawn();
        assert_eq!(world.resource::<TestFlush>().0, 12);
    }

    #[derive(Resource)]
    struct TestVec(Vec<&'static str>);

    #[derive(Component)]
    #[component(on_add = ord_a_hook_on_add, on_insert = ord_a_hook_on_insert, on_replace = ord_a_hook_on_replace, on_remove = ord_a_hook_on_remove)]
    struct OrdA;

    fn ord_a_hook_on_add(mut world: DeferredWorld, HookContext { entity, .. }: HookContext) {
        world.resource_mut::<TestVec>().0.push("OrdA hook on_add");
        world.commands().entity(entity).insert(OrdB);
    }

    fn ord_a_hook_on_insert(mut world: DeferredWorld, HookContext { entity, .. }: HookContext) {
        world
            .resource_mut::<TestVec>()
            .0
            .push("OrdA hook on_insert");
        world.commands().entity(entity).remove::<OrdA>();
        world.commands().entity(entity).remove::<OrdB>();
    }

    fn ord_a_hook_on_replace(mut world: DeferredWorld, _: HookContext) {
        world
            .resource_mut::<TestVec>()
            .0
            .push("OrdA hook on_replace");
    }

    fn ord_a_hook_on_remove(mut world: DeferredWorld, _: HookContext) {
        world
            .resource_mut::<TestVec>()
            .0
            .push("OrdA hook on_remove");
    }

    fn ord_a_observer_on_add(_trigger: Trigger<OnAdd, OrdA>, mut res: ResMut<TestVec>) {
        res.0.push("OrdA observer on_add");
    }

    fn ord_a_observer_on_insert(_trigger: Trigger<OnInsert, OrdA>, mut res: ResMut<TestVec>) {
        res.0.push("OrdA observer on_insert");
    }

    fn ord_a_observer_on_replace(_trigger: Trigger<OnReplace, OrdA>, mut res: ResMut<TestVec>) {
        res.0.push("OrdA observer on_replace");
    }

    fn ord_a_observer_on_remove(_trigger: Trigger<OnRemove, OrdA>, mut res: ResMut<TestVec>) {
        res.0.push("OrdA observer on_remove");
    }

    #[derive(Component)]
    #[component(on_add = ord_b_hook_on_add, on_insert = ord_b_hook_on_insert, on_replace = ord_b_hook_on_replace, on_remove = ord_b_hook_on_remove)]
    struct OrdB;

    fn ord_b_hook_on_add(mut world: DeferredWorld, _: HookContext) {
        world.resource_mut::<TestVec>().0.push("OrdB hook on_add");
        world.commands().queue(|world: &mut World| {
            world
                .resource_mut::<TestVec>()
                .0
                .push("OrdB command on_add");
        });
    }

    fn ord_b_hook_on_insert(mut world: DeferredWorld, _: HookContext) {
        world
            .resource_mut::<TestVec>()
            .0
            .push("OrdB hook on_insert");
    }

    fn ord_b_hook_on_replace(mut world: DeferredWorld, _: HookContext) {
        world
            .resource_mut::<TestVec>()
            .0
            .push("OrdB hook on_replace");
    }

    fn ord_b_hook_on_remove(mut world: DeferredWorld, _: HookContext) {
        world
            .resource_mut::<TestVec>()
            .0
            .push("OrdB hook on_remove");
    }

    fn ord_b_observer_on_add(_trigger: Trigger<OnAdd, OrdB>, mut res: ResMut<TestVec>) {
        res.0.push("OrdB observer on_add");
    }

    fn ord_b_observer_on_insert(_trigger: Trigger<OnInsert, OrdB>, mut res: ResMut<TestVec>) {
        res.0.push("OrdB observer on_insert");
    }

    fn ord_b_observer_on_replace(_trigger: Trigger<OnReplace, OrdB>, mut res: ResMut<TestVec>) {
        res.0.push("OrdB observer on_replace");
    }

    fn ord_b_observer_on_remove(_trigger: Trigger<OnRemove, OrdB>, mut res: ResMut<TestVec>) {
        res.0.push("OrdB observer on_remove");
    }

    #[test]
    fn command_ordering_is_correct() {
        let mut world = World::new();
        world.insert_resource(TestVec(Vec::new()));
        world.add_observer(ord_a_observer_on_add);
        world.add_observer(ord_a_observer_on_insert);
        world.add_observer(ord_a_observer_on_replace);
        world.add_observer(ord_a_observer_on_remove);
        world.add_observer(ord_b_observer_on_add);
        world.add_observer(ord_b_observer_on_insert);
        world.add_observer(ord_b_observer_on_replace);
        world.add_observer(ord_b_observer_on_remove);
        let _entity = world.spawn(OrdA).id();
        let expected = [
            "OrdA hook on_add", // adds command to insert OrdB
            "OrdA observer on_add",
            "OrdA hook on_insert", // adds command to despawn entity
            "OrdA observer on_insert",
            "OrdB hook on_add", // adds command to just add to this log
            "OrdB observer on_add",
            "OrdB hook on_insert",
            "OrdB observer on_insert",
            "OrdB command on_add", // command added by OrdB hook on_add, needs to run before despawn command
            "OrdA observer on_replace", // start of despawn
            "OrdA hook on_replace",
            "OrdA observer on_remove",
            "OrdA hook on_remove",
            "OrdB observer on_replace",
            "OrdB hook on_replace",
            "OrdB observer on_remove",
            "OrdB hook on_remove",
        ];
        world.flush();
        assert_eq!(world.resource_mut::<TestVec>().0.as_slice(), &expected[..]);
    }

    #[test]
    fn entity_world_mut_clone_and_move_components() {
        #[derive(Component, Clone, PartialEq, Debug)]
        struct A;

        #[derive(Component, Clone, PartialEq, Debug)]
        struct B;

        #[derive(Component, Clone, PartialEq, Debug)]
        struct C(u32);

        #[derive(Component, Clone, PartialEq, Debug, Default)]
        struct D;

        let mut world = World::new();
        let entity_a = world.spawn((A, B, C(5))).id();
        let entity_b = world.spawn((A, C(4))).id();

        world.entity_mut(entity_a).clone_components::<B>(entity_b);
        assert_eq!(world.entity(entity_a).get::<B>(), Some(&B));
        assert_eq!(world.entity(entity_b).get::<B>(), Some(&B));

        world.entity_mut(entity_a).move_components::<C>(entity_b);
        assert_eq!(world.entity(entity_a).get::<C>(), None);
        assert_eq!(world.entity(entity_b).get::<C>(), Some(&C(5)));

        assert_eq!(world.entity(entity_a).get::<A>(), Some(&A));
        assert_eq!(world.entity(entity_b).get::<A>(), Some(&A));
    }

    #[test]
    fn entity_world_mut_clone_with_move_and_require() {
        #[derive(Component, Clone, PartialEq, Debug)]
        #[require(B)]
        struct A;

        #[derive(Component, Clone, PartialEq, Debug, Default)]
        #[require(C(|| C(3)))]
        struct B;

        #[derive(Component, Clone, PartialEq, Debug, Default)]
        #[require(D)]
        struct C(u32);

        #[derive(Component, Clone, PartialEq, Debug, Default)]
        struct D;

        let mut world = World::new();
        let entity_a = world.spawn(A).id();
        let entity_b = world.spawn_empty().id();

        world.entity_mut(entity_a).clone_with(entity_b, |builder| {
            builder
                .move_components(true)
                .without_required_components(|builder| {
                    builder.deny::<A>();
                });
        });

        assert_eq!(world.entity(entity_a).get::<A>(), Some(&A));
        assert_eq!(world.entity(entity_b).get::<A>(), None);

        assert_eq!(world.entity(entity_a).get::<B>(), None);
        assert_eq!(world.entity(entity_b).get::<B>(), Some(&B));

        assert_eq!(world.entity(entity_a).get::<C>(), None);
        assert_eq!(world.entity(entity_b).get::<C>(), Some(&C(3)));

        assert_eq!(world.entity(entity_a).get::<D>(), None);
        assert_eq!(world.entity(entity_b).get::<D>(), Some(&D));
    }

    #[test]
    #[cfg(feature = "track_location")]
    fn update_despawned_by_after_observers() {
        let mut world = World::new();

        #[derive(Component)]
        #[component(on_remove = get_tracked)]
        struct C;

        static TRACKED: OnceLock<&'static Location<'static>> = OnceLock::new();
        fn get_tracked(world: DeferredWorld, HookContext { entity, .. }: HookContext) {
            TRACKED.get_or_init(|| {
                world
                    .entities
                    .entity_get_spawned_or_despawned_by(entity)
                    .unwrap()
            });
        }

        #[track_caller]
        fn caller_spawn(world: &mut World) -> (Entity, &'static Location<'static>) {
            let caller = Location::caller();
            (world.spawn(C).id(), caller)
        }
        let (entity, spawner) = caller_spawn(&mut world);

        assert_eq!(
            spawner,
            world
                .entities()
                .entity_get_spawned_or_despawned_by(entity)
                .unwrap()
        );

        #[track_caller]
        fn caller_despawn(world: &mut World, entity: Entity) -> &'static Location<'static> {
            world.despawn(entity);
            Location::caller()
        }
        let despawner = caller_despawn(&mut world, entity);

        assert_eq!(spawner, *TRACKED.get().unwrap());
        assert_eq!(
            despawner,
            world
                .entities()
                .entity_get_spawned_or_despawned_by(entity)
                .unwrap()
        );
    }

    #[test]
    fn with_component_activates_hooks() {
        use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

        #[derive(Component, PartialEq, Eq, Debug)]
        #[component(immutable)]
        struct Foo(bool);

        static EXPECTED_VALUE: AtomicBool = AtomicBool::new(false);

        static ADD_COUNT: AtomicU8 = AtomicU8::new(0);
        static REMOVE_COUNT: AtomicU8 = AtomicU8::new(0);
        static REPLACE_COUNT: AtomicU8 = AtomicU8::new(0);
        static INSERT_COUNT: AtomicU8 = AtomicU8::new(0);

        let mut world = World::default();

        world.register_component::<Foo>();
        world
            .register_component_hooks::<Foo>()
            .on_add(|world, context| {
                ADD_COUNT.fetch_add(1, Ordering::Relaxed);

                assert_eq!(
                    world.get(context.entity),
                    Some(&Foo(EXPECTED_VALUE.load(Ordering::Relaxed)))
                );
            })
            .on_remove(|world, context| {
                REMOVE_COUNT.fetch_add(1, Ordering::Relaxed);

                assert_eq!(
                    world.get(context.entity),
                    Some(&Foo(EXPECTED_VALUE.load(Ordering::Relaxed)))
                );
            })
            .on_replace(|world, context| {
                REPLACE_COUNT.fetch_add(1, Ordering::Relaxed);

                assert_eq!(
                    world.get(context.entity),
                    Some(&Foo(EXPECTED_VALUE.load(Ordering::Relaxed)))
                );
            })
            .on_insert(|world, context| {
                INSERT_COUNT.fetch_add(1, Ordering::Relaxed);

                assert_eq!(
                    world.get(context.entity),
                    Some(&Foo(EXPECTED_VALUE.load(Ordering::Relaxed)))
                );
            });

        let entity = world.spawn(Foo(false)).id();

        assert_eq!(ADD_COUNT.load(Ordering::Relaxed), 1);
        assert_eq!(REMOVE_COUNT.load(Ordering::Relaxed), 0);
        assert_eq!(REPLACE_COUNT.load(Ordering::Relaxed), 0);
        assert_eq!(INSERT_COUNT.load(Ordering::Relaxed), 1);

        let mut entity = world.entity_mut(entity);

        let archetype_pointer_before = &raw const *entity.archetype();

        assert_eq!(entity.get::<Foo>(), Some(&Foo(false)));

        entity.modify_component(|foo: &mut Foo| {
            foo.0 = true;
            EXPECTED_VALUE.store(foo.0, Ordering::Relaxed);
        });

        let archetype_pointer_after = &raw const *entity.archetype();

        assert_eq!(entity.get::<Foo>(), Some(&Foo(true)));

        assert_eq!(ADD_COUNT.load(Ordering::Relaxed), 1);
        assert_eq!(REMOVE_COUNT.load(Ordering::Relaxed), 0);
        assert_eq!(REPLACE_COUNT.load(Ordering::Relaxed), 1);
        assert_eq!(INSERT_COUNT.load(Ordering::Relaxed), 2);

        assert_eq!(archetype_pointer_before, archetype_pointer_after);
    }
}
