<div align="center">

# ⎈ Helm

**AI 코딩 에이전트 CLI와 조화를 이루는 가볍고 네이티브한 크로스플랫폼 대시보드-터미널.**

Helm의 터미널 안에서 Claude Code, Codex, opencode를 실행하면, 대시보드가 각
세션의 실시간 상태(진행 상황, 작업, 도구, 플러그인, 토큰 컨텍스트)를 터미널
바로 옆에 표시합니다. 에이전트 시대를 위한 tmux/cmux입니다.

<sub>`Tauri (Rust)` · `WebView2 / WKWebView / WebKitGTK` · `ConPTY / PTY` · `xterm.js + WebGL` · Electron 미사용</sub>

[English](README.md) · **한국어**

<img src="docs/screenshot.png" alt="Helm screenshot" width="900">

</div>

---

## 왜 필요한가

현대의 코딩 에이전트는 풍부하고 구조화된 출력(작업 목록, 도구 호출, 토큰 예산,
다단계 계획)을 내놓는 CLI입니다. 평범한 터미널은 이 모든 것을 한 덩어리의
텍스트 스크롤백으로 납작하게 만들어 버립니다.

Helm은 에이전트를 **실제 터미널**에서 실행하고(그래서 셸에서와 정확히 똑같이
모든 것이 동작합니다), **동시에** 그 구조를 **실시간 대시보드**로 재구성합니다 —
에이전트는 이를 알지도, 신경 쓰지도 않습니다. 하나의 창, 여러 에이전트, 완전한
상황 인식입니다.

에이전트가 지원하면 Helm은 **네이티브 훅**을 등록해 상태가 바뀌는 즉시
푸시되도록 합니다 — 폴링이 없습니다. 지원하지 않거나(또는 훅이 잠잠해지면)
Helm은 파일시스템 이벤트로 깨어나 에이전트 자체의 세션 로그를 읽습니다. 어느
쪽이든 대시보드는 실시간으로 유지됩니다.

## 무엇을 얻는가

3분할 대시보드:

| 분할 | 표시 내용 |
|---|---|
| **왼쪽** | 프로젝트별로 묶인 모든 세션을, 상태 점과 실시간 활동 막대와 함께 표시합니다. 새 세션은 직접 고른 작업 폴더(경로 입력 또는 네이티브 폴더 탐색)에서 시작됩니다. |
| **가운데** | 세션 탭(에이전트별 아이콘, 주의 표시 링) + 실시간 터미널. **Terminal ⇄ Conversation** 토글로 트랜스크립트를 메시지 카드로 다시 렌더링하며, 모델 / 에이전트 / 추론 빠른 제어가 있는 작성기를 포함합니다. |
| **오른쪽** | 활성 세션의 실시간 상태 — 진행 상황, 작업, 도구/플러그인, 토큰 컨텍스트 — 그리고 세션별 이벤트 타임라인과 실시간 시스템 CPU/MEM. |

## 기능

| 기능 | 동작 |
|---|---|
| **즉각적 네이티브 훅** | 지원되는 각 에이전트가 라이프사이클 이벤트(도구 시작/종료, 작업 변경, 턴 완료)를 localhost 수신기로 POST하는 훅을 등록합니다 — 상태가 일어나는 즉시 갱신되고, 폴링이 전혀 없습니다. 등록은 추가 방식이며 에이전트 자체 설정이나 신뢰 저장소를 절대 건드리지 않습니다. |
| **세밀한 실시간 진행 상황** | 오른쪽 레일과 작업 보드가 현재 하위 단계를 표시합니다: 대상 파일/명령과 함께 실행 중인 도구, 강조된 진행 중 작업, 단계 카운터(예: `4/8`) — Claude, Codex, opencode 모두 해당. |
| **작업 보드** | 실행 중인 세션마다 카드 한 장: 상태, 실시간 활동, 초 단위 경과 타이머, 컨텍스트 사용량 막대, 그리고 아래에 전체 작업 목록. |
| **대화 보기** | 트랜스크립트를 메시지 카드로 다시 렌더링합니다 — user/assistant/system 역할, 접을 수 있는 추론, 펼칠 수 있는 결과를 가진 도구 호출. |
| **이미지 라이트박스 + 복사** | 메시지의 이미지는 썸네일로 렌더링되며, 클릭하면 라이트박스가 열립니다(확대, 화살표 키 탐색, Esc / 바깥 클릭으로 닫기). 각 메시지(텍스트), 각 코드 블록(코드), 각 이미지에 복사 버튼이 있습니다. |
| **작성기 빠른 제어** | 전송 버튼 위의 모델 / 에이전트 / 추론 칩. Claude의 모델 목록(Default, Opus, Opus Plan, Sonnet, Haiku)은 `/model <name>`으로 한 번에 전환됩니다. opencode는 라이브 API로 전환하고, Codex는 자체 네이티브 선택기를 엽니다. |
| **실제 모델/에이전트 전환 (opencode)** | Helm은 비어 있는 localhost 포트에서 opencode를 실행하고, API에서 라이브 모델/에이전트 목록을 읽어, 선택한 `{model, agent}`를 다음 메시지와 함께 보냅니다 — opencode가 이를 영구 저장합니다. API 실패 시 네이티브 선택기로 폴백합니다. |
| **완료된 계획 원클릭 실행** | 에이전트가 계획을 마치면 Plans 탭에 나타납니다. ▶를 클릭하면 해당 에이전트가 곧바로 구현합니다 — 별도의 작업 시작 단계가 없습니다. |
| **Claude 계정 자동 전환** | Claude 세션의 컨텍스트가 턴 경계에서 임계값을 넘으면, Helm은 (현재 자격 증명을 백업한 뒤) 다음으로 구성된 계정 프로필로 교체하고 `--continue`로 재개합니다. 게이트가 걸려 있고 옵트인 방식입니다(아래 참고). |
| **모바일 LAN 접속** | 📱 버튼이 같은 Wi-Fi에 있는 어떤 휴대폰에서든 쓸 수 있는 URL을 보여줍니다. 휴대폰은 HTTP + WebSocket으로 세션을 실시간으로 조작합니다. |
| **이미지 붙여넣기** | 클립보드에 이미지가 있을 때 `Ctrl/Cmd`+`V`를 누르면 임시 PNG를 저장하고 그 경로를 붙여넣어, Claude Code / opencode 같은 에이전트가 이를 첨부합니다. |
| **Windows-Terminal 스타일 클립보드** | `Ctrl/Cmd`+`V` / `Shift`+`Insert`로 붙여넣기. `Ctrl`+`Shift`+`C` / `Cmd`+`C`로 선택 영역 복사. 선택 영역이 없을 때 `Ctrl`+`C`는 에이전트를 중단합니다. bracketed-paste 안전. |
| **세션 자동 복원** | 작업 디렉터리 + 에이전트 + PTY 상태가 재시작과 재부팅을 넘어 유지됩니다. Claude는 `--continue`로 재개합니다. |
| **설정** | 터미널 글꼴, 커서 깜빡임, 기본 에이전트, 패널 표시, 복원 토글, 에이전트별 옵션 — localStorage에 영구 저장되고 실시간 적용됩니다. |
| **WebGL 터미널** | xterm.js가 글리프를 GPU에서 렌더링하여(DOM 폴백) 빠르고 실시간 에코되는 상호작용을 제공합니다. |

## 에이전트 지원

| 기능 | Claude Code | Codex | opencode |
|---|:---:|:---:|:---:|
| 감지 + 라벨링 | ✓ (스크립트 또는 대화형) | ✓ (타이틀 + 로그) | ✓ (타이틀 + 로그) |
| 상태 / 활동 | ✓ 훅 + 트랜스크립트 | ✓ 훅 + 로그 | ✓ 훅 + 로그 + DB |
| 세밀한 하위 단계 (도구 + 대상) | ✓ | ✓ | ✓ |
| 작업 / 할 일 | ✓ (TodoWrite) | ✓ (`update_plan`) | ✓ (DB 작업) |
| 도구 / 플러그인 | ✓ (MCP) | ✓ (타임라인) | ✓ (MCP + 플러그인) |
| 대화 보기 | ✓ 완전한 충실도 | ✓ 완전한 충실도 | ✓ ¹ (DB 재구성) |
| 추론 (사고) | ✓ 접기 가능 | ✓ 접기 가능 | ✓ 접기 가능 |
| 메시지 내 도구 호출 | ✓ 결과 포함 | ✓ 결과 포함 | ✓ 결과 포함 |
| 메시지 내 이미지 | ✓ 라이트박스 + 복사 | ✓ 라이트박스 + 복사 | ✓ 라이트박스 + 복사 |
| 토큰 컨텍스트 | ✓ | ✓ | ✓ ² (DB, 오프라인 폴백) |
| 현재 모드/모델 | — | — | ✓ (모드 + 모델 칩) |
| 실제 모델/에이전트 전환 | ✓ `/model` (원클릭) | — (네이티브 선택기) | ✓ ³ HTTP API (원클릭) |
| 계획 감지 + 실행 | ✓ (ExitPlanMode) | ✓ (`update_plan`) | ✓ (계획 모드) |
| 네이티브 훅을 통한 즉각성 | ✓ (localhost POST) | ✓ (localhost POST) | ✓ (localhost 플러그인 POST) |
| 소스 | 트랜스크립트 + 훅 | 롤아웃 로그 + 훅 | SQLite DB + 훅 + logfmt |

평범한 셸도 감지되며(**bash / pwsh / cmd / wsl**로 라벨링), CLI가 셸로 빠져나가
종료되면 세션은 자동으로 에이전트 라벨을 내려놓습니다.

> ¹ **opencode 대화**는 opencode의 SQLite 저장소에서 재구성됩니다(텍스트, 추론,
> 도구 호출) — 읽기 전용으로 열기 때문에 opencode가 실행 중이어도 동작합니다.
> logfmt 시스템 이벤트가 즉각적인 활동 줄을 제공하고, DB가 전체 대화·작업·토큰
> 컨텍스트·현재 모드/모델을 공급합니다.
>
> ² **opencode 토큰 컨텍스트**는 DB에서 읽으며, 사용할 수 없을 때는 모델 ID에서
> 유도한 모델별 컨텍스트 최댓값으로 폴백합니다.
>
> ³ **opencode 실제 전환**은 opencode 자체의 HTTP API를 사용하므로(슬래시 명령
> 없음, 네이티브 선택기 없음) 변경이 직접적이고 영구 저장됩니다.

### 계획(Plans)

에이전트가 계획을 마치면 — Claude의 ExitPlanMode, Codex의 `update_plan`, 또는
opencode의 계획 모드 — Helm은 `plan-detected` 이벤트를 내보내고 그 계획을
**Plans** 탭에 나열합니다(최신순, 최대 50개, 콘텐츠 해시로 중복 제거). ▶를
클릭하면 Helm은 "지금 구현하라"는 지시를 에이전트에게 곧바로 보냅니다:

| 에이전트 | 실행 전달 방식 |
|---|---|
| Claude Code | 터미널에 지시 입력 + Enter |
| Codex | 터미널에 지시 입력 |
| opencode | HTTP API로 메시지 전송 |

계획은 localStorage에 영구 저장되므로 목록은 재시작 후에도 유지됩니다.

### 계정 자동 전환 (Claude)

단일 Claude 세션의 토큰 컨텍스트가 턴 경계(유휴 / 대기 / 완료)에서 설정 가능한
임계값(기본 **85%**)에 도달하면, Helm은 신선한 할당량을 얻기 위해 다음으로 구성된
계정 프로필로 순환합니다:

- 먼저 현재 자격 증명을 백업하고(잠금 위험 없음), 원자적 파일 연산(임시 파일
  쓰기 → 크기 확인 → 이름 변경)을 사용해 전역 `~/.claude` 자격 증명 파일을 다음
  프로필의 것으로 교체합니다.
- `--continue`로 세션을 재개합니다.
- **게이트:** 이 기능은 `~/.claude/account-profiles/` 아래에 프로필이 **두 개
  이상** 존재할 때까지 비활성 상태로 유지됩니다. 임계값, 순환 순서, 활성화는
  Settings에 있습니다.
- **솔직한 주의사항:** Claude 자격 증명은 전역적이므로, 교체는 정확히 **하나**의
  Claude 세션만 살아 있을 때만 발동합니다. 여러 Claude 세션이 동시에 있으면
  설계상 꺼진 상태로 유지됩니다.

## 모바일 접속

상단 바의 **📱** 버튼은 같은 Wi-Fi에 있는 어떤 휴대폰에서든 바로 붙여넣을 수
있는 URL을 보여줍니다. Helm은 동일한 임베디드 UI를 HTTP로 제공하고, 직접 구현한
WebSocket(RFC 6455)으로 동일한 이벤트 스트림을 브리지하므로, 휴대폰이 세션을
실시간으로 조작합니다 — 터미널, 진행 상황, 작업, 대화.

휴대폰에서는 레이아웃이 전체 너비 단일 열로 접힙니다:

| 제어 | 동작 |
|---|---|
| **☰** | 세션 목록을 슬라이드인 드로어로 엽니다 |
| **📊** | 세션 상태 레일을 슬라이드인 드로어로 엽니다 |
| 어두워진 영역 탭 / 세션 선택 / `Esc` | 드로어를 닫습니다 |

- URL 쿼리 문자열의 **실행마다 무작위로 생성되는 페어링 토큰**이 WebSocket에
  게이트를 겁니다. 기본 포트는 **`8787`**(HTTP)과 **`8788`**(WS)이며,
  `HELM_HTTP_PORT` / `HELM_WS_PORT`로 재정의할 수 있습니다.
- **LAN 전용.** 클라우드 릴레이도 QR 코드도 없습니다 — 로컬 네트워크에서 일반
  HTTP로 제공됩니다. HTTPS/리디렉션은 지원하지 않으므로 직접 HTTP URL을
  사용하세요.
- 상단 바의 Wi-Fi 알약은 연결된 휴대폰 수를 보여주고, 휴대폰이 연결되면 📱
  버튼이 초록색으로 빛납니다.
- 휴대폰이 페이지를 불러오지 못하면, 두 기기가 같은 네트워크에 있는지, 그리고
  방화벽이 사설 네트워크에서 해당 포트를 허용하는지 확인하세요.

## 키보드 & 클립보드

`Mod` = Windows/Linux에서는 `Ctrl`, macOS에서는 `Cmd`.

**세션**

| 단축키 | 동작 |
|---|---|
| `Ctrl`+`Tab` / `Ctrl`+`Shift`+`Tab` | 다음 / 이전 세션 |
| `Mod`+`1`–`8` | 세션 1–8로 이동 |
| `Mod`+`9` | 마지막 세션으로 이동 |
| `Mod`+`Shift`+`T` | 새 세션 |
| `Mod`+`Shift`+`W` | 활성 세션 닫기 |

**보기 & 터미널**

| 단축키 | 동작 |
|---|---|
| `Mod`+`Shift`+`M` | Terminal ⇄ Conversation 토글 |
| `Mod`+`Shift`+`K` | 터미널 스크롤백 지우기 |
| `Mod`+`Shift`+`U` | 알림 패널 |
| `Mod`+`,` | 설정 열기 |
| `Mod`+`=` / `Mod`+`-` | 글꼴 크기 키우기 / 줄이기 (0.5 pt 단위) |
| `Mod`+`0` | 글꼴 크기 초기화 (12.5 pt) |
| `Ctrl`+`Shift`+`?` | 키보드 단축키 모달 |

**클립보드**

| 단축키 | 동작 |
|---|---|
| `Mod`+`V` / `Shift`+`Insert` | 터미널에 붙여넣기 (bracketed-paste 안전) |
| `Ctrl`+`Shift`+`C` / `Cmd`+`C` | 선택 영역 복사 (텍스트가 선택되었을 때) |
| `Ctrl`+`C` | 선택 영역 없음 → 에이전트 중단 |
| `Mod`+`V` (클립보드에 이미지) | 임시 PNG 저장, 그 경로 붙여넣기 |

단축키는 의도적으로 `Ctrl`+`Shift`+`<문자>` / `Ctrl`+`<숫자>` 공간을 사용하므로,
터미널은 readline 등을 위해 일반 `Ctrl`+`<문자>`를 그대로 유지합니다.

## 요구 사항

| OS | 웹뷰 | 추가 빌드 의존성 |
|---|---|---|
| Windows 10/11 | WebView2 (최신 빌드에 사전 설치됨) | 없음 |
| macOS | WKWebView (내장) | 없음 |
| Linux | WebKitGTK 4.1 | `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `librsvg2-dev`, `build-essential` |

빌드에는 안정 버전 Rust 툴체인(edition 2021, Rust 1.77+)이 필요합니다.

## 소스에서 빌드

```bash
cd src-tauri
cargo build --release
```

출력물:

| OS | 바이너리 |
|---|---|
| Windows | `src-tauri\target\release\helm.exe` |
| macOS / Linux | `src-tauri/target/release/helm` |

프런트엔드는 **빌드 시점에 임베드**되므로(`generate_context!`), `ui/` 아래의 어떤
변경도 반영되려면 재빌드가 필요합니다. 전체 빌드 없이 프런트엔드 구문만
점검하려면:

```bash
node --check ui/app.js
```

CI는 `windows-latest`, `macos-latest`, `ubuntu-latest`에서 릴리스 바이너리를
빌드하며(Rust stable + Node 20, Linux 웹뷰 의존성 설치됨), 각 빌드 전에
프런트엔드 구문 점검을 실행합니다.

## 구성

**설정**(앱 내, localStorage에 영구 저장, 실시간 적용):

| 설정 | 기본값 | 비고 |
|---|---|---|
| `fontSize` | `12.5` | 터미널 글꼴 크기 (포인트) |
| `cursorBlink` | `true` | 커서 깜빡임 |
| `defaultAgent` | `"ask"` | 새 세션용 에이전트 |
| `statsInterval` | `2000` | 시스템 통계 갱신 (ms) |
| `restoreSessions` | `true` | 실행 시 세션 복원 |
| `show.progress` / `show.todos` / `show.tools` / `show.usage` / `show.timeline` | `true` | 오른쪽 레일의 분할별 표시 |
| `opencode.notifyTurnDone` | `true` | 턴 완료 시 토스트 |
| `opencode.notifyAwaiting` | `true` | 입력 대기 시 토스트 |
| `opencode.showConversation` | `true` | 대화 보기에 DB 대화 렌더링 |
| `opencode.apiSwitch` | `true` | HTTP-API 모델/에이전트 전환 노출 |
| `multiAccount.enabled` | `false` | Claude 계정 자동 전환 (프로필 ≥2개 필요) |
| `multiAccount.thresholdPct` | `85` | 전환을 발동하는 컨텍스트 % |
| `multiAccount.order` | `[]` | 프로필 순환 시퀀스 |

**환경 변수:**

| 변수 | 기본값 | 용도 |
|---|---|---|
| `HELM_HTTP_PORT` | `8787` | 모바일 UI가 제공되는 포트 (LAN) |
| `HELM_WS_PORT` | `8788` | 모바일 WebSocket 브리지가 수신하는 포트 |
| `HELM_USAGE_PORT` | 미설정 | 계정 사용량 JSON 엔드포인트용 로컬 포트. 이 값이 설정되지 않으면 사용량 패널은 숨겨진 채로 유지됩니다 |

각각은 관대하게 파싱되며, 미설정이거나 유효하지 않으면 기본값으로 폴백합니다.

## 작동 방식

모든 에이전트는 PTY ID를 키로 하는 하나의 이벤트 스트림으로 정규화되며,
프런트엔드는 이를 일반적으로 렌더링합니다:

| 이벤트 | 페이로드 | 의미 |
|---|---|---|
| `pty-data:{id}` | `{ b64 }` | 원시 터미널 출력 (base64) |
| `pty-exit:{id}` | — | 프로세스 종료됨 |
| `agent-progress:{id}` | `{ status, activity, todos[], tools[], context, mode?, model?, sid?, current_tool?, active_todo_index?, step_display? }` | 오른쪽 레일 실시간 상태 |
| `conv-msg:{id}` | `{ id, role, ts, text, thinking?, tool_calls[], usage?, images[] }` | 대화 메시지 |
| `conv-tool:{id}` | `{ id, name, status, result }` | 도구 호출 결과 |
| `conv-reset:{id}` | — | 대화 보기 비우기 |
| `plan-detected:{id}` | `{ plan_id, title, description, ... }` | 에이전트가 계획을 마침 (콘텐츠 해시로 중복 제거) |
| `agent-turn-done:{id}` | `{ title, model }` | opencode 턴 완료 (알림 트리거) |
| `mobile-clients` | `{ count }` | 연결된 휴대폰 수 변경됨 |

**에이전트별 데이터 채널:**

- **Claude Code** — 상태, 활동, 할 일, 도구를 위해 `~/.claude/projects/<slug>/`
  아래의 JSONL 트랜스크립트를 읽습니다(`<slug>`는 작업 디렉터리에서 모든
  비영숫자 문자를 `-`로 매핑한 것). 토큰 컨텍스트는 있을 때는 OMC HUD 캐시에서,
  그 외에는 트랜스크립트에서 옵니다. 프로젝트 훅이 라이프사이클 이벤트를 즉시
  푸시하며, 훅이 활성화되면 워처는 상태/활동/할 일 방출을 멈추고 훅이 주도하도록
  합니다.
- **Codex** — 파일시스템 이벤트로 깨어날 때 `~/.codex/sessions/…` 아래의 JSONL
  롤아웃 로그를 읽습니다(거의 즉각적). 롤아웃 로그는 작업과 도구의 진실
  원천이며, 훅은 더 이른 상태 알림만 전달합니다.
- **opencode** — opencode의 `opencode.db`(SQLite, WAL, 읽기 전용이며 리더
  친화적)를 읽어 전체 대화, 작업 목록, 실시간 토큰 컨텍스트, 현재 모드/모델을
  재구성합니다. Helm은 또한 비어 있는 localhost 포트에서 opencode를 실행하고 그
  HTTP API를 사용해 실시간 모델/에이전트 전환을 수행합니다. 플러그인 훅이 즉각적
  상태를 위해 이벤트를 POST합니다.

**훅**은 모두 시작 시 바인딩된 단일 localhost 수신기로 POST하며, 수신기는 보고된
CWD를 PTY에 매핑하고 `agent-progress`를 방출합니다. 워처는 훅이 비활성이거나
미등록일 때마다 실시간 폴백으로 남아 있습니다.

**모바일 브리지**는 이 정확한 이벤트 스트림을 HTTP + WebSocket으로 미러링합니다.
백엔드 이벤트는 연결된 모든 휴대폰으로 팬아웃되고, 휴대폰은 동일한 디스패치
경로를 통해 명령을 다시 보냅니다.

## 프로젝트 구조

| 경로 | 내용 |
|---|---|
| `src-tauri/src/main.rs` | ConPTY 생성 + I/O, 시스템 통계, Tauri 명령, 백그라운드 폴러, Claude 계정 프로필 |
| `src-tauri/src/agent_watch.rs` | 에이전트별 로그 워처(Claude JSONL, Codex JSONL, opencode SQLite) → 정규화된 `agent-progress` + `conv-*` 이벤트 |
| `src-tauri/src/hook_server.rs` | localhost 훅 수신기 + 에이전트별 훅 등록. CWD → PTY 매핑 |
| `src-tauri/src/mobile.rs` | LAN HTTP + WebSocket 서버, 실행마다 페어링 토큰, 이벤트 브로드캐스트 |
| `ui/index.html` · `ui/app.js` · `ui/styles.css` | 바닐라 JS IIFE 프런트엔드(`window.App`) — 스토어, 터미널 마운트/IO, 렌더, 키보드/클립보드 |
| `ui/vendor/xterm/` | 벤더링된 xterm.js(Terminal) + Fit / WebGL / WebLinks 애드온 |

## 문제 해결

| 증상 | 해결 |
|---|---|
| 빈 / 흰 창 | 강제 종료 후 WebView 캐시 손상. Windows에서는 `%LOCALAPPDATA%\com.helm.app`를 삭제하고 다시 실행하세요. |
| 오른쪽 분할이 비어 있음 | 부모 셸이 아니라 세션 **안에서** 에이전트를 실행하고, 세션의 작업 폴더가 에이전트가 실제로 실행되는 곳과 일치하는지 확인하세요 — 작업 디렉터리가 로그 조회 키입니다. |
| 휴대폰이 페이지를 못 불러옴 | 두 기기가 같은 Wi-Fi에 있는지, 방화벽이 사설 네트워크에서 해당 포트를 허용하는지 확인하세요. HTTPS/리디렉션은 지원하지 않으므로 직접 HTTP URL을 사용하세요. |
| 붙여넣기가 안 됨 | 먼저 터미널에 포커스를 둔 뒤 `Mod`+`V` 또는 `Shift`+`Insert`를 누르세요. |
| 사용량 패널 없음 | `HELM_USAGE_PORT`가 설정되지 않으면 정상입니다. |

## 로드맵

- LAN을 넘어서는 모바일 접속(선택적, 옵트인 클라우드 릴레이).
- 더 많은 에이전트 — 하나를 추가하는 일은 정규화된 이벤트 스트림을 방출하는
  워처를 작성하는 것뿐입니다.

## 라이선스

MIT © kalhintz
