@echo off
setlocal
set "AGENT_GIT_BASH="
if exist "%ProgramFiles%\Git\bin\bash.exe" set "AGENT_GIT_BASH=%ProgramFiles%\Git\bin\bash.exe"
if not defined AGENT_GIT_BASH if exist "%ProgramFiles(x86)%\Git\bin\bash.exe" set "AGENT_GIT_BASH=%ProgramFiles(x86)%\Git\bin\bash.exe"
if not defined AGENT_GIT_BASH for %%I in (bash.exe) do set "AGENT_GIT_BASH=%%~$PATH:I"
if not defined AGENT_GIT_BASH (
  echo agent-git: Git for Windows bash.exe was not found 1>&2
  exit /b 1
)
set "AGENT_WRAPPER=%~dp0agent-git"
set "AGENT_WRAPPER=%AGENT_WRAPPER:\=/%"
"%AGENT_GIT_BASH%" "%AGENT_WRAPPER%" %*
exit /b %ERRORLEVEL%
