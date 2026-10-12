# Drek's Fleet (level 17): the batch C checklist

Batch C's last planet, done 2026-10-04 after Quartu. Before this pass the Fleet ran ported code for its sliding
lasers 99, its turrets, the ship sections and their HUD, the energy barriers 78, the electrified water 655 and the
rippling water meshes 1405..1407 (the last three shared with Quartu), and the shared crates, bolts and checkpoints. The
census after Quartu left the seven classes below without a port; with them every placed class of every level has one.

## Classes without a port (census 2026-10-04)

| class | what | status |
|---|---|---|
| 835 | the floating mines: bob, blow up when hit or touched, with a splash | **ported** (`units::fleet_small`, from the disassembly) |
| 1380 | the lift pads that turn over for the Magneboots | **ported** (`units::fleet_lift`) |
| 1382 | the crew: alert, salute the Hologuise, chase and punch inside their area | **ported** (`units::fleet_crew`) |
| 1448 | the shuttle: Ratchet's ship flies him between its two landing spots | **ported** (`units::fleet_shuttle`; Blarg's 1109 with two routes) |
| 1470 | the help director, with the dive and party skill points | **ported** (`units::help_fleet`, from the disassembly) |
| 1562 | the scene thrusters | **ported** (`units::fleet_small`) |
| 1772 | the space backdrop: the death height on and off the fleet, the glowing lanes and nebulae | **ported** (`units::fleet_sky`) |

## Shared-system changes in this pass

- The cinematic channel got the follow camera's turn toward a point (`0x313b48`,
  `cinematic::follow_turn_toward_point`), for the help director's look.
- The backdrop's animation tables live in the unit globals (`Globals::fleet_sky`), fresh on each level load as the
  overlay's data is.
- The backdrop sets the level's death height through the hero writes' `death_z` (as Kalebo's race host does).

## Notes

- [L] The backdrop's draw turns depth writes off (GS TEST 0x513f1); the port's effect quads write no depth. Its
  0x16d3e4 = 8 192 000 is the view context's far plane (`UpdateViewContext` projects every later frame with it):
  `Services::view_far`, followed by the projection and the tfrag cull (2026-10-12; the nebula squares hang 1000 past
  the lanes' far end, y −7000, which the draw also writes).
- The window glass 1907, 1908, 1910, 1911, 1913, 1918 (2026-10-12): Kalebo's 1952 code with their own draws (FX 0x30,
  FIX 0x20, the reflection map at factor 4), ported as sea rows (`water::sea::fleet_glass_ref`). They were matched to
  1952 by code identity and loaded nothing (six copies on the level); a port on its own reference level now wins its
  exact address in `LevelPorts` (which also gave Quartu's wave meshes 1408/1409/1565/1567 their own rows back from
  the Fleet's 1405, and Quartu's 1428 its own port).
- [L] A mine touched by Ratchet's capsule skips its sphere query, so the game tests a stale hit moby; the port takes the
  touch as a blast. 0x13f590 is not kept by the port.
- Headless screenshots in this session came out black on every level: a capture problem here, not a regression.

## Open

- The user's QA: docs/test-requests/2026-10-04-fleet.md.
