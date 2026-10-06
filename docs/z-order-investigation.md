# Preview 5: native Z-order investigation

Preview 5 was reported unchanged. This is an investigation, not a resolved user
incident or a new release. Whether the report concerns effects, desktop input,
or both, and whether Firefox's exclusion was removed, are not established.

## First native result

Run: https://github.com/zufall-upon/monocle/actions/runs/37442484677
Source: acac016 (investigate-z-order branch).

Existing 45 tests passed. The added native fixture failed in its shared hidden
owner case. It creates actual Windows HWNDs, a root/tint/grain ownership chain,
three ownerless GPU-style windows, ignored/active/background roles, a visible
owned popup, and a topmost settings window. It uses actual foreground changes.

For independent application windows, lowering the background produces a
transient `root > background > GPU` gap. Re-pinning the GPU windows repairs it.
The fixture's settled state meets the background-below-all-GPUs invariant.

For two app windows sharing a hidden owner, setup instead produces:

    settings > ignored > grain > tint > root > GPU2 > GPU1 > GPU0
      > active > popup > background

The active window was wrongly lowered along with the background. This is a
reproduced ownership interaction, but it is the **opposite direction** to the
reported background-above-root symptom. It does not establish the cause of
Firefox/Tablacus behavior on the user's machine.

## Comparison experiment

The extended fixture adds ignored-app foreground -> settings -> ignored-app
return, uses the production setup anchor policy, logs the hidden owner rank,
and compares SWP_NOOWNERZORDER on demotions only, promotions only, and both.
Root/GPU positioning stays fixed. Production code is not changed by this test.

## Scope boundaries

Window roles and placement operations model Preview 5. The full tracker and its
process classification are not executed: all synthetic HWNDs share one process
and thread. There are no real Firefox/Tablacus windows, no physical three-monitor
setup, no application-driven re-raising, and no WGC capture in this fixture.
The GPU-style HWNDs have no DComp content. The separate existing opaque DComp
input fixture remains required; neither fixture proves real Explorer gestures.

A new stable Preview 5 report after a real app -> Firefox/Tablacus -> settings
transition can distinguish retained-sharp policy from an actual placement
violation using retained_anchors, retained_sharp, sharp_reason, ignored entries,
transition history and root/GPU ranks. No user settings are automatically removed.

## Repeated reconciliation result

Run: https://github.com/zufall-upon/monocle/actions/runs/37444215539
Test source: 331a065. All 46 tests passed, including the original opaque DComp
input fixture. This is a comparison test: it explicitly expects the baseline
ownership defect and requires the candidate to satisfy all invariants.

The final model repeats the production-order operation snapshot four times:
GPU pin, enumerate targets in their original z-order, check each target's current
position, lower background or raise sharp, then the following tracker GPU pin.
It also uses the real setup anchor policy for normal -> ignored -> settings ->
ignored transitions. These are native window operations, not only pure tests.

| SWP_NOOWNERZORDER applied to | Result in this fixture |
| --- | --- |
| Neither direction (Preview 5) | Active wrongly below GPU after setup and repeated reconciliation |
| Background demotion only | No recorded ordering/input violations |
| Sharp promotion only | Setup violation remains |
| Both directions | No recorded ordering/input violations |

Baseline repeated reconciliation repeatedly ended with `root > active > popup >
background > hidden-helper > GPU`, after which GPU pin placed the active window
under the blur again. It therefore did not converge to the intended sharp/background
partition. The log does **not** show stable background-above-root after complete
reconciliation. Do not describe it as reproducing the user's persistent symptom.

Demotion-only is the smallest supported candidate for this discovered owner
interaction. Before adopting it, production push_below_overlay and setup demotion
should share the tested native helper, and the fixture should call that same
helper. Visible popup ordering, multiple sharp groups, and cross-process behavior
remain additional boundaries. No production placement changes were made in this
investigation and no release was created. The application's input styles, root/GPU
placement, capture pipeline and user settings are unchanged.
