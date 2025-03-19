use bevy::prelude::{Mat4, Vec4};
use bevy_ecs::prelude::*;

#[derive(Component)]
struct A(Mat4);
#[derive(Component)]
struct B(Vec4);
fn main() {
    let mut world = World::default();
    let entity_count = 3;
    for _ in 0..entity_count {
        world
            .spawn((A(Mat4::default()), B(Vec4::default())))
            .with_children(|parent| {
                parent.spawn((A(Mat4::default()), B(Vec4::default())));
            });
    }
    let ents = world
        .iter_entities()
        .map(|e| e.id())
        .take(entity_count as usize)
        .collect::<Vec<_>>();
    println!("Ents before {:?}", ents);
    ents.iter().for_each(|e| {
        let mut ent = world.entity_mut(*e);
        println!("{} is despawned {}", ent.id(), ent.is_despawned());
        ent.despawn();
    });
    let ents = world.iter_entities().map(|e| e.id()).collect::<Vec<_>>();
    println!("Ents after {:?}", ents);
}
