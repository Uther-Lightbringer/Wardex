#!/usr/bin/env bash
# 春江花月夜 · 张若虚
#
# 春江潮水连海平，海上明月共潮生。
# 滟滟随波千万里，何处春江无月明！
# 江流宛转绕芳甸，月照花林皆似霰；
# 空里流霜不觉飞，汀上白沙看不见。
# 江天一色无纤尘，皎皎空中孤月轮。
# 江畔何人初见月？江月何年初照人？
# 人生代代无穷已，江月年年望相似。
# 不知江月待何人，但见长江送流水。
# 白云一片去悠悠，青枫浦上不胜愁。
# 谁家今夜扁舟子？何处相思明月楼？
# 可怜楼上月裴回，应照离人妆镜台。
# 玉户帘中卷不去，捣衣砧上拂还来。
# 此时相望不相闻，愿逐月华流照君。
# 鸿雁长飞光不度，鱼龙潜跃水成文。
# 昨夜闲潭梦落花，可怜春半不还家。
# 江水流春去欲尽，江潭落月复西斜。
# 斜月沉沉藏海雾，碣石潇湘无限路。
# 不知乘月几人归，落月摇情满江树。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Check for --no-env flag
NO_ENV=false
ARGS=()
for arg in "$@"; do
  if [[ "$arg" == "--no-env" ]]; then
    NO_ENV=true
  else
    ARGS+=("$arg")
  fi
done

if [[ "$NO_ENV" == "true" ]]; then
  # Unset API keys (see packages/ai/src/env-api-keys.ts)
  unset ANTHROPIC_API_KEY
  unset ANTHROPIC_OAUTH_TOKEN
  unset OPENAI_API_KEY
  unset GEMINI_API_KEY
  unset GROQ_API_KEY
  unset CEREBRAS_API_KEY
  unset XAI_API_KEY
  unset OPENROUTER_API_KEY
  unset ZAI_API_KEY
  unset MISTRAL_API_KEY
  unset MINIMAX_API_KEY
  unset MINIMAX_CN_API_KEY
  unset AI_GATEWAY_API_KEY
  unset OPENCODE_API_KEY
  unset COPILOT_GITHUB_TOKEN
  unset GH_TOKEN
  unset GITHUB_TOKEN
  unset HF_TOKEN
  unset GOOGLE_APPLICATION_CREDENTIALS
  unset GOOGLE_CLOUD_PROJECT
  unset GCLOUD_PROJECT
  unset GOOGLE_CLOUD_LOCATION
  unset AWS_PROFILE
  unset AWS_ACCESS_KEY_ID
  unset AWS_SECRET_ACCESS_KEY
  unset AWS_SESSION_TOKEN
  unset AWS_REGION
  unset AWS_DEFAULT_REGION
  unset AWS_BEARER_TOKEN_BEDROCK
  unset AWS_CONTAINER_CREDENTIALS_RELATIVE_URI
  unset AWS_CONTAINER_CREDENTIALS_FULL_URI
  unset AWS_WEB_IDENTITY_TOKEN_FILE
  unset AZURE_OPENAI_API_KEY
  unset AZURE_OPENAI_BASE_URL
  unset AZURE_OPENAI_RESOURCE_NAME
  echo "Running without API keys..."
fi

"$SCRIPT_DIR/node_modules/.bin/tsx" --tsconfig "$SCRIPT_DIR/tsconfig.json" "$SCRIPT_DIR/packages/coding-agent/src/cli.ts" ${ARGS[@]+"${ARGS[@]}"}
