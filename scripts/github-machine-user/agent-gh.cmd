@echo off
setlocal
set "AGENT_GIT_BASH="
for %%I in (bash.exe) do set "AGENT_GIT_BASH=%%~$PATH:I"
if not defined AGENT_GIT_BASH if exist "%ProgramFiles%\Git\bin\bash.exe" set "AGENT_GIT_BASH=%ProgramFiles%\Git\bin\bash.exe"
if not defined AGENT_GIT_BASH (
  echo agent-gh: Git for Windows bash.exe was not found 1>&2
  exit /b 1
)
"%AGENT_GIT_BASH%" "%~dp0agent-gh" %*
exit /b %ERRORLEVEL%
