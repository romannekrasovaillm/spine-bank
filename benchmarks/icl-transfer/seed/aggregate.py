import json, re, glob, os
h1 = json.load(open('h1_results.json')); ts = json.load(open('tool_stats.json'))
def evs(run): return [json.loads(l) for l in open(f'runs/{run}/events.jsonl')]
agg = {}
for run, r in h1.items():
    arm = r['arm']; rows = r['rows']
    later = [x for x in rows[1:] if x['attempted']]
    t1 = rows[0]
    homes = [k for k in ts if k == run or k.startswith(run + '-')]
    tok = sum(ts[k]['usage']['prompt'] + ts[k]['usage']['completion'] for k in homes)
    secs = sum(e.get('secs', 0) for e in evs(run) if e['kind'] in ('spine_session', 'coder_task'))
    complete = all(x['attempted'] for x in rows) and all(x.get('final_pass', True) for x in rows) if arm not in ('C',) else True
    d = dict(arm=arm, complete=complete, n_later=len(later),
             later_hidden_fail=sum(x['first_hidden_fail'] for x in later),
             later_first_pass=sum(x['first_pass'] for x in later),
             later_dispatch=sum(x['dispatches'] for x in later),
             t1_dispatch=t1.get('dispatches'), t1_hidden=t1.get('first_hidden_fail'),
             conv_prompts=sum(1 for x in later if x.get('first_prompt_has_conventions')),
             tokens=tok, secs=round(secs), calls=sum(sum(ts[k]['calls'].values()) for k in homes))
    agg[run] = d
    print(run, d)
json.dump(agg, open('aggregate.json', 'w'), ensure_ascii=False, indent=1)
