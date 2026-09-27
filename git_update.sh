#!/bin/bash

# Use all command-line arguments as the commit message.
# If none are supplied, fall back to the old timestamp-based message.
if [ "$#" -gt 0 ]; then
    MSG="$*"
else
    MSG="Committed At $(TZ=Asia/Calcutta date)"
fi

git status
echo "++++++++++++++"

git add .
echo "++++++++++++++"

git commit -m "$MSG"
echo "++++++++++++++"

git push -u origin main
