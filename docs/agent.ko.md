# Web agent

[English](agent.md) | **한국어**

개발자가 아닌 팀원도 web tool 에서 변경을 요청 가능. Coding agent (Claude Agent SDK 또는 Codex SDK)
가 임시 수정안에서 작업하고, 팀은 다른 수정안과 같은 방식으로 보고 merge 함 ([변경 흐름](workflow.ko.md)).

## 켜기

Docker 설치에서 동작함. Agent overlay 를 추가:

```bash
docker compose -f compose.yaml -f compose.agent.yaml up -d --build
# public mode: docker compose -f compose.yaml -f compose.public.yaml -f compose.agent.yaml up -d --build
```

추가되는 것 2가지:

- **agent** service: 이 컴퓨터의 Docker socket 으로 run 마다 container 1개 실행.
- **charet-agent-run** image: 그 container 들이 쓰는 image (agent · `charet` CLI · charet skill, 약 1.5 GB).

계정은 **설정 › Web agent** (관리자) 에서 입력. Provider 별 (Claude = Anthropic, Codex = OpenAI) 로 API key · 구독 로그인 · 둘 다 사용 가능:

- **API key**: 붙여넣고 **확인** 클릭.
  - Provider 가 key 가 동작하는지 알려줌. 동작하는 key 만 **저장** 가능.
  - Server key 로 암호화해 저장, 끝 4자리만 표시.
- **Claude / ChatGPT 로 로그인**: 새 창에서 provider 의 로그인 page 가 열림.
  - ChatGPT: page 에 표시된 code 를 provider page 에 입력. 완료는 charet 이 알아서 감지.
  - Claude: 로그인 후 provider page 에 표시된 code 를 charet 에 붙여넣기.
  - 로그인 정보는 암호화해 보관. Run 시작 전에 갱신하고, run container 에는 수명이 짧은 access token 만 전달. Refresh token 은 server 밖으로 나가지 않음.
- 둘 다 설정하면 agent 가 쓸 쪽을 선택.
- `compose.yaml` 옆 `.env`: `CHARET_ANTHROPIC_API_KEY=…` (Claude) · `CHARET_OPENAI_API_KEY=…` (Codex) · `CHARET_GOOGLE_API_KEY=…` (추후용, 아직 Google 로 실행되는 agent 없음).
  Page 입력보다 우선, page 에서 변경 불가.

같은 page 에서 기본 agent 와 agent 별 model 설정 (빈 값 = agent 기본 model).

구독 로그인은 provider 자체 command line tool 의 로그인 방식을 사용함. 개인 설치 또는 소규모 신뢰 팀용. 팀 작업을 한 사람의 구독에 올리기 전에 provider 약관 확인 필요.

## 요청하기

- 화면에서 옆 패널을 **Agent** 로 전환.
  - 특정 요소에 대한 요청이면 먼저 그 요소를 클릭함. 화면 · theme · 요소 정보 (component chain · source 위치 · text) 가 요청과 함께 전달됨.
  - 이미지 (screenshot · 스케치) 는 이미지 버튼 · 붙여넣기 · drop 으로 첨부.
- **작업할 곳**: 새 임시 수정안 (요청 내용이 제목) 또는 내 임시 수정안 중 선택.
- 진행 상황 실시간 표시: 사용한 도구, agent 답변, 종료 상태.
  - **중지** 시 run 종료. 이미 저장한 변경은 수정안에 남음.
- 수정안 화면 **Agent** tab: 대화 기록 유지 + 다음 요청.
  - 그 수정안의 이전 요청 · 답변이 함께 전달되므로 "조금 더 크게" 같은 요청도 맥락대로 이해함.
- 저장할 때마다 임시 수정안에 push 되므로 수정안 화면의 before/after 로 변경 확인 가능.
  - 괜찮으면 **검토 요청** → 편집자가 merge.

동작 규칙:

- 수정안 하나에는 한 번에 run 1개만 실행. 두 번째 요청은 대기함.
- Server 전체 동시 실행 수: 기본 3 (`CHARET_AGENT_MAX_RUNNING`).
- 새 임시 수정안에서 아무것도 저장하지 않고 끝난 run 은 빈 수정안을 남기지 않음.

## 비용

- **설정 › Web agent** 아래 비용 목록: 사람별 실행 수 · token · 비용.
- Claude: 비용을 직접 보고함.
- Codex: charet-agent 에 준 가격표로 계산 (`CHARET_AGENT_PRICES`, model 별 100만 token 당 달러 JSON).
  - 예: `{"gpt-5.5": {"input": 1.25, "output": 10, "cachedInput": 0.125}}`
  - 가격표에 없는 model 로 실행된 건은 가격 미상으로 따로 셈.
- 사용 한도 없음. 한도는 model provider 쪽에서 설정함.

## Run 격리 방식

- Run 마다 별도 container:
  - capability 전부 제거, 권한 상승 금지.
  - memory · CPU · process 수 제한 (`CHARET_AGENT_MEMORY_GB`, `CHARET_AGENT_CPUS`).
  - 시간 제한 (`CHARET_AGENT_TIMEOUT_MIN`, 기본 30분).
- Network 는 internal.
  - 바깥으로 나가는 길은 2개뿐: agent service 를 거쳐 server **agent door** 로 가는 중계, model API 에만 tunnel 을 여는 HTTPS proxy (`CHARET_AGENT_ALLOW_HOSTS`, 기본 `api.anthropic.com,api.openai.com,chatgpt.com`).
    - 구독 로그인으로 실행한 run 이 연결에 실패하면 보고된 host 를 이 목록에 추가.
  - Internet 에서 package 설치 불가.
- Agent door (`CHARET_AGENT_BIND`, host 에 publish 안 됨) 에서 run token 의 권한:
  - 실행 중에만, 그 project 에서만 유효.
  - 허용 동작: project 읽기, 자기 수정안에 push, check, capture.
  - 불가: merge, comment, 다른 project 조회, 관리 API.
  - Push 는 *이름 · agent* 로 표시됨.
- Vendor key (로그인이면 수명이 짧은 access token) 는 run container 안으로 전달됨 (agent 실행에 필요). charet 전용 key 를 따로 만들어 쓰면 그 key 만 폐기 가능.
  - Run 은 로그인을 스스로 갱신할 수 없으므로, 남은 수명이 35분 미만이면 charet 이 먼저 갱신함.
- Agent service 는 Docker socket (이 컴퓨터의 root 권한) 을 가짐. Run container 에서는 두 중계와 token 으로 보호된 API 외에는 접근 불가.

## 설정

| 변수 | 위치 | 기본값 | |
|---|---|---|---|
| `CHARET_ANTHROPIC_API_KEY`, `CHARET_OPENAI_API_KEY`, `CHARET_GOOGLE_API_KEY` | `.env` | — | key (또는 설정 › Web agent) |
| `CHARET_AGENT_MAX_RUNNING` | `.env` | 3 | 동시 실행 수 |
| `CHARET_AGENT_NETWORK` | `.env` | `charet-agent-runs` | run 용 internal network |
| `CHARET_AGENT_TIMEOUT_MIN` | agent service | 30 | 최대 실행 시간 |
| `CHARET_AGENT_MEMORY_GB`, `CHARET_AGENT_CPUS` | agent service | 4, 2 | run 당 |
| `CHARET_AGENT_PRICES` | agent service | — | Codex 가격표 |
| `CHARET_AGENT_ALLOW_HOSTS` | agent service | model API | proxy 허용 목록 |
