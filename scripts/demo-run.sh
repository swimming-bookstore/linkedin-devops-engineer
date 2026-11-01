#!/bin/sh
# Open the job desk. Walk every guest page for the past 7 days.
cd "$(dirname "$0")/.."
exec ./target/debug/linkedin-devops-jobs -k 'DevOps Engineer' -l 'Hong Kong'
