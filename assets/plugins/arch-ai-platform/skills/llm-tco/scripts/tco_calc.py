#!/usr/bin/env python3
"""tco_calc.py — стоимость владения LLM: внешний API против собственного инференса (on-prem или аренда GPU).
Только stdlib. Все цены задаёт пользователь — скрипт не знает актуальных прайсов.

Нагрузка (одно из):
  --in-mtok / --out-mtok        миллионов входных/выходных токенов в месяц
  --rps --in --out              средние запросы/с и длины (токенов)
  --cache-hit 0.6               доля входных токенов из prompt/prefix-кэша
  --peak-to-avg 3               отношение пика к среднему (для сайзинга своего парка)

API (можно несколько):  --api "Модель-A:in=3,out=15,cache=0.3"   (валюта за 1M токенов)
Свой инференс:
  --gpu-throughput "out=1500,in=20000"   ток/с на 1 GPU при вашем SLA (из capacity_plan.py/бенчмарка)
  аренда:   --gpu-hour 2.5                         (за GPU-час; reserved/on-demand)
  on-prem:  --gpu-capex 35000 --amort-years 4 --power-kw 1.2 --pue 1.3 --kwh 0.1
            --hosting-gpu-month 150 --software-gpu-year 2000
  общее:    --ops-fte 1.5 --fte-year 120000 --redundancy 1 --target-util 0.7 --currency USD

Примеры:
  python tco_calc.py --in-mtok 3000 --out-mtok 400 --cache-hit 0.5 --api "big:in=3,out=15,cache=0.3" \
      --api "small:in=0.3,out=1.2,cache=0.03" --gpu-throughput "out=1800,in=25000" --gpu-hour 2.5 --ops-fte 1
  python tco_calc.py --rps 3 --in 4000 --out 500 --api "api:in=2,out=8" --gpu-throughput "out=1500,in=20000" \
      --gpu-capex 30000 --amort-years 4 --power-kw 1.2 --kwh 0.12 --ops-fte 2 --fte-year 100000
"""
import argparse
import json
import math
import signal
import sys

SEC_MONTH = 730 * 3600


def kv(s):
    d = {}
    for item in s.split(","):
        k, v = item.split("=")
        d[k.strip()] = float(v)
    return d


def api_cost(in_m, out_m, cache, p):
    pc = p.get("cache", p["in"])
    return in_m * (1 - cache) * p["in"] + in_m * cache * pc + out_m * p["out"]


def selfhost(in_m, out_m, o):
    thr = kv(o.gpu_throughput)
    prefill_share = 1 - o.cache_hit * o.prefix_reuse  # кэш префиксов экономит prefill
    gpu_sec = out_m * 1e6 / thr["out"] + in_m * 1e6 * prefill_share / thr["in"]
    avg_gpus = gpu_sec / SEC_MONTH
    need = max(o.min_gpus, math.ceil(avg_gpus * o.peak_to_avg / o.target_util) + o.redundancy)
    if o.gpu_hour is not None:
        infra = need * o.gpu_hour * 730
        parts = {"аренда GPU": infra}
    else:
        capex_m = need * o.gpu_capex / (o.amort_years * 12)
        power_m = need * o.power_kw * 730 * o.pue * o.kwh
        parts = {"амортизация": capex_m, "электричество": power_m,
                 "размещение": need * o.hosting_gpu_month}
    parts["ПО/поддержка"] = need * o.software_gpu_year / 12
    parts["персонал"] = o.ops_fte * o.fte_year / 12
    total = sum(parts.values())
    return dict(gpus=need, avg_gpus=avg_gpus, util=avg_gpus / need if need else 0, parts=parts, total=total)


def main():
    if hasattr(signal, "SIGPIPE"):
        signal.signal(signal.SIGPIPE, signal.SIG_DFL)
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--in-mtok", type=float); p.add_argument("--out-mtok", type=float)
    p.add_argument("--rps", type=float); p.add_argument("--in", dest="inp", type=float); p.add_argument("--out", type=float)
    p.add_argument("--cache-hit", type=float, default=0.0)
    p.add_argument("--prefix-reuse", type=float, default=1.0, help="доля кэш-попаданий API, повторяемых prefix caching у себя")
    p.add_argument("--peak-to-avg", type=float, default=3.0)
    p.add_argument("--api", action="append", default=[])
    p.add_argument("--gpu-throughput"); p.add_argument("--gpu-hour", type=float)
    p.add_argument("--gpu-capex", type=float, default=0); p.add_argument("--amort-years", type=float, default=4)
    p.add_argument("--power-kw", type=float, default=1.2, help="на 1 GPU с долей сервера/сети")
    p.add_argument("--pue", type=float, default=1.3); p.add_argument("--kwh", type=float, default=0.1)
    p.add_argument("--hosting-gpu-month", type=float, default=0); p.add_argument("--software-gpu-year", type=float, default=0)
    p.add_argument("--ops-fte", type=float, default=1.0); p.add_argument("--fte-year", type=float, default=100000)
    p.add_argument("--redundancy", type=int, default=1); p.add_argument("--min-gpus", type=int, default=1)
    p.add_argument("--target-util", type=float, default=0.7)
    p.add_argument("--currency", default="USD"); p.add_argument("--json", action="store_true")
    o = p.parse_args()

    if o.in_mtok is None:
        if None in (o.rps, o.inp, o.out):
            sys.exit("Задайте --in-mtok/--out-mtok или --rps --in --out")
        o.in_mtok = o.rps * o.inp * SEC_MONTH / 1e6
        o.out_mtok = o.rps * o.out * SEC_MONTH / 1e6
    o.out_mtok = o.out_mtok or 0.0
    apis = []
    for a in o.api:
        name, _, rest = a.partition(":")
        apis.append((name, kv(rest)))
    if not apis and not o.gpu_throughput:
        sys.exit("Нужно хотя бы одно --api или --gpu-throughput")
    if o.gpu_throughput and o.gpu_hour is None and not o.gpu_capex:
        sys.exit("Для своего инференса задайте --gpu-hour (аренда) или --gpu-capex (покупка)")
    cur = o.currency
    res = dict(workload=dict(in_mtok=o.in_mtok, out_mtok=o.out_mtok, cache_hit=o.cache_hit), api={}, selfhost=None)
    print("Нагрузка в месяц: вход %.0fM ток. (из кэша %.0f%%), выход %.0fM ток.; пик/среднее %.1f" % (
        o.in_mtok, o.cache_hit * 100, o.out_mtok, o.peak_to_avg))
    tot_m = o.in_mtok + o.out_mtok
    print("\n%-24s %14s %16s" % ("Вариант", "в месяц, " + cur, "за 1M ток., " + cur))
    for name, pr in apis:
        c = api_cost(o.in_mtok, o.out_mtok, o.cache_hit, pr)
        res["api"][name] = c
        print("%-24s %14.0f %16.3f" % ("API " + name, c, c / tot_m))
    sh = None
    if o.gpu_throughput:
        sh = selfhost(o.in_mtok, o.out_mtok, o)
        res["selfhost"] = sh
        print("%-24s %14.0f %16.3f   (%d GPU, средняя загрузка парка %.0f%%)" % (
            "Свой инференс", sh["total"], sh["total"] / tot_m, sh["gpus"], sh["util"] * 100))
        print("  структура: " + ", ".join("%s %.0f" % (k, v) for k, v in sh["parts"].items() if v))

    if sh and apis:
        print("\nТочка безубыточности (масштабируем нагрузку при той же структуре):")
        for name, pr in apis:
            be = None
            for mult in [x / 100 for x in range(5, 5001, 5)]:
                s2 = selfhost(o.in_mtok * mult, o.out_mtok * mult, o)
                if s2["total"] <= api_cost(o.in_mtok * mult, o.out_mtok * mult, o.cache_hit, pr):
                    be = mult
                    break
            if be is None:
                print("  vs %-18s свой инференс дороже во всём диапазоне до ×50 текущей нагрузки" % name)
            elif be <= 1:
                print("  vs %-18s уже выгоднее свой: окупается с ×%.2f текущей нагрузки (~%.0fM ток./мес)" % (name, be, tot_m * be))
            else:
                print("  vs %-18s свой выгоднее при нагрузке ≥ ×%.2f текущей (~%.0fM ток./мес)" % (name, be, tot_m * be))
            res.setdefault("break_even", {})[name] = be
        print("\nЧувствительность своего инференса к нагрузке (стоимость за 1M ток.):")
        print("  " + "  ".join("×%-5g" % m for m in (0.25, 0.5, 1, 2, 4, 8)))
        print("  " + "  ".join("%-6.3f" % (selfhost(o.in_mtok * m, o.out_mtok * m, o)["total"] / (tot_m * m))
                               for m in (0.25, 0.5, 1, 2, 4, 8)))
    print("\nНе учтено: разработка и интеграция, стоимость простоя, качество моделей (сравнивайте эвалами),"
          "\nскидки за объём, налоги/пошлины, сроки поставки GPU, регуляторные требования к размещению данных.")
    if o.json:
        print(json.dumps(res, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
