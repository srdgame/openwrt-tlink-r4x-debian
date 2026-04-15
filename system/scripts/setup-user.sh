#!/bin/sh

set -e

echo "I: create user"
useradd -s /bin/bash admin

echo "I: set user password"
echo "admin:admin" | chpasswd
usermod -aG sudo admin
