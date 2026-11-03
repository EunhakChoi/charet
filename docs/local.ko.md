# charet 로컬 실행

[English](local.md) | **한국어**

charet 는 service 3개로 동작함: **charet-server** (API · 웹 도구 · 미리보기), **charet-workspace**
(프로젝트 코드 install · build), **charet-render** (Chromium: 캡처 · 검사). 주소로 신뢰하는 사람 없음:
소유자는 **관리자 link**, 팀원은 token 으로 입장, 나머지는 방문자. 로컬 모드에서 port 는 이 컴퓨터의
loopback 에만 열리고, 다른 사람은 공유로 입장.

## Docker 로 실행 (권장)

Docker · Compose v2 필요.

```bash
git clone https://github.com/EunhakChoi/charet.git
cd charet
./start.sh
```

`start.sh` 가 `docker compose up -d` 실행 → server 준비 대기 → **관리자 link**
(`http://localhost:5280/?t=<token>`) 출력. 설치 직후에는 브라우저로 열기까지. 이 link 를 한 번 열면
(`127.0.0.1` 아닌 `localhost`) 그 브라우저는 소유자 (첫 관리자) 로 계속 로그인. 언제든 `./start.sh` 를
다시 실행하면 시작 + link 재출력. `charet setup` (CLI 로그인) · `docker compose exec server charet-server
admin-token` (token 만) 으로도 확인 가능. link 없이 **http://localhost:5280** 접속 시 방문자 (기본:
보기만), 소유자가 처음 들어오기 전까지 그 화면에 link 찾는 방법 안내. `docker compose up -d` 만 써도
동작 — server 첫 시작 시 log (`docker compose logs server`) 에 link 1회 출력. Windows 는 WSL · Git Bash
에서 `start.sh` 실행, 또는 위 명령 직접 사용.

미리보기는 다른 site 인 `127.0.0.1:5281` 에서 제공 → 프로젝트 코드가 도구의 cookie 를 읽을 수 없음.

첫 image build 는 시간이 걸림 (Rust · 웹 도구 compile).

| 설정 (환경 변수 또는 `.env`) | 기본값 | |
|---|---|---|
| `CHARET_PORT` | `5280` | 웹 도구 · API |
| `CHARET_PREVIEW_PORT` | `5281` | 미리보기 |
| `CHARET_UPDATE_CHECK` | `true` | 하루 1회 새 버전 확인 (유일한 외부 호출) |
| `CHARET_MERGE_CHECK` | `true` | 채택 전 merge 결과 build · 화면 검사 (미리보기 없는 server 는 `false`) |
| `CHARET_TZ` | `UTC` | 렌더 화면의 time zone (예: `Asia/Seoul`) |
| `CHARET_DATA_DIR` | (volume `charet_data`) | 프로젝트 · 댓글 · 캡처를 둘 이 기기의 폴더 |
| `CHARET_USER` | `charet` | server 실행 user. `CHARET_DATA_DIR` 사용 시 폴더 주인 (`uid:gid`) |

데이터는 Docker volume 에 보관:

| Volume | 내용 |
|---|---|
| `charet_data` | 프로젝트 (git) · 버전 · 댓글 · 캡처 · 미리보기 서명 key |
| `charet_workspace` | 의존성 cache · build 결과 (지워도 됨, 필요할 때 다시 build) |
| `charet_secrets` | service 끼리 쓰는 token |

```bash
docker compose logs -f server        # log
docker compose up -d --build         # `git pull` 후 갱신
docker compose down                  # 중지 (데이터 유지)
docker compose down -v               # 중지 + 데이터 전부 삭제 (CHARET_DATA_DIR 폴더는 제외)

# 프로젝트 · 댓글 · 캡처 백업
docker run --rm -v charet_data:/data -v "$PWD":/backup debian:trixie-slim \
  tar -czf /backup/charet-data.tgz -C /data .
```

volume 대신 원하는 폴더에 두려면 폴더를 만들고 `CHARET_DATA_DIR` 로 지정. `CHARET_USER` 는 그 폴더
주인으로 (git 은 다른 user 소유 repo 를 거부함):

```bash
mkdir -p ~/charet-data
printf 'CHARET_DATA_DIR=%s\nCHARET_USER=%s\n' ~/charet-data "$(id -u):$(id -g)" >> .env
docker compose up -d
```

## 팀 · 공유

헤더의 **팀** 에 팀원 목록. 이름 · 권한 (관리자 · 편집 · 댓글 · 보기) 으로 팀원을 추가하고
**token** 또는 token 이 담긴 link (`…/?t=<token>`) 전달 → 한 번 열면 그 브라우저는 계속 로그인.
token 은 팀 화면에서 언제든 보기 · 복사 가능, **token 재발급** 시 이전 token 중지. 같은 token 으로
다른 브라우저가 들어오면 이전 브라우저는 로그아웃 (CLI 는 유지). token 이 없는 사람은 **방문자**:
임의 이름으로 팀 공개 프로젝트 사용. 팀 화면의 *방문자* 로 보기 · 댓글 가능 · token 필요 중 선택,
프로젝트별로 **접근** 에서 따로 설정 가능.

`localhost:5280` 포함 모든 주소에서 같은 규칙. 소유자 (*소유자* 표시) 는 첫 관리자이며 삭제 · 강등 불가.

다른 기기의 접속은 두 번째 port 인 **원격 문** (`127.0.0.1:5284`, `CHARET_REMOTE_BIND`) 으로 받음.
**설정 › 접근** (공유) 에서 접속 방식 선택:

- **Tailscale 사설망** — 같은 tailnet 사람만 (Tailscale, 또는 Headscale 은 tailnet 안 HTTP).
  charet 가 `tailscale serve` 실행 후 기기 이름 표시.
- **Tailscale Funnel** — 주소를 아는 누구나 기기의 `ts.net` 이름으로 접속. 기기에 Funnel 이 꺼져
  있으면 관리 화면 link 표시.
- **ngrok** — 서버가 ngrok tunnel 직접 실행 (ngrok 프로그램 불필요). ngrok dashboard 의 authtoken
  입력 (서버에 보관, 다시 표시 안 함) 및 있으면 ngrok 도메인 입력 → 주소 고정.
  `CHARET_NGROK_AUTHTOKEN` 도 가능.
- **내 주소** — 직접 준비한 tunnel · reverse proxy (ngrok, cloudflared, nginx 등) 를
  `127.0.0.1:5284` 로 연결 (5280 아님: 원격 문을 거쳐야 미리보기가 같은 주소에서 sandbox 로 제공됨).
- **같은 네트워크** — 원격 문을 이 기기의 LAN 주소에도 열기 (HTTP). 시험용: link 에 IP 노출, token
  암호화 없이 전송. (Docker 는 대신 5284 port 를 LAN 에 publish.)

공유 중에는 link (팀원 link · 알림) 가 공유 주소 사용. `charet doctor` 로 응답 여부 확인. Tailscale
방식은 charet-server 가 도는 곳에 `tailscale` CLI 필요 (`CHARET_TAILSCALE`, `CHARET_TAILSCALE_SOCKET`).
Docker 에서는 **내 주소** 선택 후 host 에서 `tailscale serve --bg http://127.0.0.1:5284` (또는
`tailscale funnel …`) 실행. 미리보기도 같은 주소로 sandbox 상태로 제공. 팀 전용 server 는
[외부 공개 모드](public.ko.md) 참고.

## CLI

CLI 는 아직 배포 전 — clone 에서 실행 (Node.js 24+, pnpm 12):

```bash
pnpm install
alias charet="node $PWD/apps/charet-cli/src/main.ts"
charet doctor                          # node · git · server · 내부 service 점검
charet setup                           # 소유자로 로그인, 관리자 link 출력, skill 설치 제안
charet init demo                       # 권장 template 으로 새 프로젝트
charet import ../my-app                # 또는 기존 앱 (쓰는 workspace package 포함)
```

`CHARET_URL` 로 다른 server 지정 (기본 `http://127.0.0.1:5280`).

## Docker 없이 실행

Rust (stable) · Node.js 24 · pnpm 12 · git · Chromium 필요
(또는 `pnpm --filter charet-web exec playwright install chromium-headless-shell`).

```bash
make setup
cargo build --release -p charet-server -p charet-render
pnpm --filter charet-web run build
node apps/charet-cli/src/main.ts serve    # server + workspace + render, Ctrl-C 로 모두 중지
```

데이터는 `~/.charet` (`CHARET_HOME`) 에 저장.

## 보안 참고

- 방문자 이상의 작업은 모든 요청에 팀원 token (또는 session) 필요. 그래도 port 는 loopback 에 두고
  원격 문으로 공유 — 팀 전용 server 는 [외부 공개 모드](public.ko.md) 참고.
- 프로젝트 코드 (의존성 install · build) 는 charet-workspace 안에서만 실행, install script 끔 · 최소
  환경 변수. 미리보기는 별도 origin 의 browser 에서 실행, 서명된 짧은 수명 token 사용.
- install · build 때 code 를 실행하는 파일 (package manifest · lockfile · build 설정) 변경은 관리자
  승인 대기. 승인 전 미리보기는 `main` 의 파일 사용 ([변경 흐름](workflow.ko.md#채택)).
- 외부 호출은 하루 1회 새 버전 확인뿐 (`CHARET_UPDATE_CHECK=false` 로 끔). 코드 · 화면 · 사용 정보
  전송 없음.

## 문제 해결

- `charet doctor` 가 무엇이 문제인지와 시작 방법을 알려 줌.
- 도구에서 미리보기가 비어 보임: 도구를 `127.0.0.1` 아닌 `http://localhost:<port>` 로 열기.
- port 충돌: `CHARET_PORT` / `CHARET_PREVIEW_PORT` 지정 후 `docker compose up -d` 다시 실행.

## 임시 이름 `board` 에서 올리기

- server 가 첫 실행 때 data 폴더의 예전 `board.db` 를 그대로 이어받음.
- 환경 변수 이름이 `CHARET_*` 로 바뀜 (이전 `BOARD_*`). `.env` 에서 이름 변경 필요.
- Docker Compose project 이름이 `charet` 로 바뀌어 volume 이 새로 생김. data 를 폴더 (`CHARET_DATA_DIR`) 에 두거나, 전환 전 예전 `board_data` volume 을 복사.
- 각 프로젝트: `.board/` → `.charet/`, `@board/sdk` → `@charet/sdk`, vite plugin `board()` → `charet()` 로 바꾼 뒤 `charet push`.
