#!/bin/bash
# judge_loop.sh — автодосуд ячеек по мере их появления (glm-канал догоняется
# асинхронно). Каждая итерация пересканирует ячейки: судит всё, у чего есть
# answer.md >= 500B и нет judge.json. Выход — когда неосуждённых 0 И
# glm-драйвер мёртв (больше новых ответов не будет).
set -u
BENCH=/home/user/spine-bank/benchmarks/platformv-arch-bench
RUNS=/home/user/experiments/0909-platformv-arch-benchmark
LOG=$RUNS/logs/judge_loop.log
export PVBENCH_RUNS=$RUNS
export PVBENCH_JUDGE_URL=https://api.deepseek.com/v1/chat/completions
export PVBENCH_JUDGE_MODEL=deepseek-v4-pro
export PVBENCH_JUDGE_KEY_ENV=DEEPSEEK_API_KEY
export PVBENCH_JUDGE_K=1
export PVBENCH_JUDGE_WORKERS=24
cd "$BENCH"
while true; do
  left_before=$(find $RUNS/cells -maxdepth 2 -name answer.md -size +500c ! -path "*/work/*" | wc -l)
  judged=$(find $RUNS/cells -maxdepth 2 -name judge.json | wc -l)
  echo "$(date '+%F %T') итерация: ответов=$left_before отсуждено=$judged" >> "$LOG"
  env -u HTTP_PROXY -u HTTPS_PROXY -u http_proxy -u https_proxy -u ALL_PROXY -u all_proxy \
    python3 runners/judge.py >> "$LOG" 2>&1
  left=$(comm -23 \
    <(find $RUNS/cells -maxdepth 2 -name answer.md -size +500c ! -path "*/work/*" -printf "%h\n" | sort) \
    <(find $RUNS/cells -maxdepth 2 -name judge.json -printf "%h\n" | sort) | wc -l)
  driver=$(pgrep -fc "runners/run_matrix" || true)
  echo "$(date '+%F %T') осталось неосуждённых: $left, драйверов: $driver" >> "$LOG"
  if [ "$left" -eq 0 ] && [ "$driver" -eq 0 ]; then
    echo "$(date '+%F %T') всё отсуждено, драйверы мертвы — выход" >> "$LOG"
    break
  fi
  sleep 180
done
