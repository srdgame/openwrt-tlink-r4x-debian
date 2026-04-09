
#!/bin/bash

# 获取用户指定的目录，如果没有提供则使用当前目录
TARGET_DIR="${1:-.}"

# 检查目录是否存在
if [ ! -d "$TARGET_DIR" ]; then
    echo "错误: 目录 '$TARGET_DIR' 不存在"
    exit 1
fi

# 枚举指定目录下的所有文件并输出内容
for file in "$TARGET_DIR"/*; do
    # 检查是否为普通文件（排除目录）
    if [ -f "$file" ]; then
        cat "$file"
    fi
done
