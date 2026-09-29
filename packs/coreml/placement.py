"""Where Core ML will run each model in a pack, from MLComputePlan.

    python placement.py PACK_DIR

Prints JSON: per model, op count and estimated-cost share per compute device.
Exits 1 if any ANE-layout model places < 95 % of its ops on the Neural Engine,
because then the lane is a slow CPU lane that only looks like an ANE lane.
The plan reflects the machine it runs on: a VM with no Neural Engine reports
everything on CPU/GPU, so CI records this report but does not gate on it.
"""
import collections
import json
import os
import sys
import threading

import CoreML
import Foundation

MIN_ANE_SHARE = 0.95
LANES = ('ane', 'gpu')
UNITS = {'CPU_AND_NE': CoreML.MLComputeUnitsCPUAndNeuralEngine,
         'CPU_AND_GPU': CoreML.MLComputeUnitsCPUAndGPU}


def device(usage):
    # MLNeuralEngineComputeDevice -> NeuralEngine
    return type(usage.preferredComputeDevice()).__name__.removeprefix('ML').removesuffix('ComputeDevice')


def plan_of(path, units, function):
    """Core ML's own MLComputePlan for one function of a compiled model:
    coremltools' wrapper plans only the default function of a multifunction
    model, so it is asked through the framework, with the function named."""
    config = CoreML.MLModelConfiguration.alloc().init()
    config.setComputeUnits_(UNITS[units])
    if function:
        config.setFunctionName_(function)
    got, done = {}, threading.Event()

    def handler(plan, error):
        got.update(plan=plan, error=error)
        done.set()

    CoreML.MLComputePlan.loadContentsOfURL_configuration_completionHandler_(
        Foundation.NSURL.fileURLWithPath_(path), config, handler)
    done.wait()
    if got['plan'] is None:
        sys.exit(f'no compute plan for {path}:{function}: {got["error"]}')
    return got['plan']


def main():
    pack = sys.argv[1]
    manifest = json.load(open(os.path.join(pack, 'manifest.json')))
    report, ok = [], True
    for m in [dict(manifest[sub], S=S, path=manifest[sub]['file'].replace('{S}', str(S)))
              for sub in LANES for S in manifest[sub]['buckets']]:
        function = m.get('function', '').replace('{S}', str(m['S'])) or None
        plan = plan_of(os.path.join(pack, m['path']), m['compute_units'], function)
        functions = plan.modelStructure().program().functions()
        fn = functions[function] if function else next(iter(functions.values()))
        ops, cost, off_ane = collections.Counter(), collections.Counter(), collections.Counter()
        for op in fn.block().operations():
            usage = plan.computeDeviceUsageForMLProgramOperation_(op)
            if usage is None:  # constants: no device
                continue
            d = device(usage)
            ops[d] += 1
            c = plan.estimatedCostOfMLProgramOperation_(op)
            cost[d] += c.weight() if c else 0
            if m['layout'] == 'ane' and d != 'NeuralEngine':
                off_ane[str(op.operatorName())] += 1
        n, w = sum(ops.values()) or 1, sum(cost.values()) or 1
        share = ops['NeuralEngine'] / n
        passed = m['layout'] != 'ane' or share >= MIN_ANE_SHARE
        ok &= passed
        report.append(dict(model=f"{m['path']}:{function or 'main'}", compute_units=m['compute_units'],
                           ops=dict(ops),
                           op_share={k: round(v / n, 4) for k, v in ops.items()},
                           cost_share={k: round(v / w, 4) for k, v in cost.items()},
                           ops_off_ane=dict(off_ane.most_common(8)), passed=passed))
    print(json.dumps(dict(min_ane_share=MIN_ANE_SHARE, passed=ok, models=report), indent=1))
    sys.exit(0 if ok else 1)


if __name__ == '__main__':
    main()
