#!/bin/bash

# This file is for deploying Faint Light to SSH servers.
# Mainly used during benchmarks. You can ignore it.

# Load environment variables from .local.env
if [ -f ".local.env" ]; then
    source .local.env
else
    echo "Error: .local.env file not found!"
    echo "Please create .local.env with default remote configuration."
    exit 1
fi

# Function to discover available remotes
discover_remotes() {
    local remotes=()
    for env_file in .remote.*.env; do
        if [ -f "$env_file" ] && [ "$env_file" != ".remote.sample.env" ]; then
            local remote_name=$(basename "$env_file" .env | sed 's/^\.remote\.//')
            remotes+=("$remote_name")
        fi
    done
    echo "${remotes[@]}"
}

# Function to load remote configuration
load_remote_config() {
    local remote_name="$1"
    local env_file=".remote.${remote_name}.env"
    
    if [ ! -f "$env_file" ]; then
        echo "Error: Remote configuration file '$env_file' not found!"
        return 1
    fi
    
    # Load the remote configuration
    source "$env_file"
    
    # Set the deployment variables
    export DEPLOY_SSH_USER="$REMOTE_SSH_USER"
    export DEPLOY_SSH_HOST="$REMOTE_SSH_HOST"
    export DEPLOY_SSH_PORT="$REMOTE_SSH_PORT"
    export DEPLOY_SSH_KEY="$REMOTE_SSH_KEY"
    export DEPLOY_REMOTE_PATH="$REMOTE_PATH"
    
    return 0
}

# Check if 'deploy' argument is provided
if [ "$1" != "deploy" ]; then
    echo "Usage: $0 deploy [remote]"
    echo "This script will rsync the current project to the remote server."
    
    # Discover and list available remotes
    available_remotes=($(discover_remotes))
    if [ ${#available_remotes[@]} -eq 0 ]; then
        echo "No remote configurations found. Create .remote.*.env files to define remotes."
    else
        echo "Available remotes: ${available_remotes[*]}"
    fi
    echo "Default remote: $DEPLOY_REMOTE"
    exit 1
fi

# Determine which remote to use
REMOTE=${2:-$DEPLOY_REMOTE}

# Load remote configuration
if ! load_remote_config "$REMOTE"; then
    echo "Error: Failed to load configuration for remote '$REMOTE'"
    exit 1
fi

# Check if required environment variables are set
if [ -z "$DEPLOY_SSH_USER" ] || [ -z "$DEPLOY_SSH_HOST" ] || [ -z "$DEPLOY_SSH_PORT" ] || [ -z "$DEPLOY_SSH_KEY" ] || [ -z "$DEPLOY_REMOTE_PATH" ]; then
    echo "Error: Missing required environment variables for remote '$REMOTE'"
    echo "Required variables: REMOTE_SSH_USER, REMOTE_SSH_HOST, REMOTE_SSH_PORT, REMOTE_SSH_KEY, REMOTE_PATH"
    exit 1
fi

# Expand the SSH key path
SSH_KEY_EXPANDED=$(eval echo $DEPLOY_SSH_KEY)

# Check if SSH key exists
if [ ! -f "$SSH_KEY_EXPANDED" ]; then
    echo "Error: SSH key not found at $SSH_KEY_EXPANDED"
    exit 1
fi

echo "Deploying project to $REMOTE remote..."
echo "Target: $DEPLOY_SSH_USER@$DEPLOY_SSH_HOST:$DEPLOY_SSH_PORT"
echo "Remote path: $DEPLOY_REMOTE_PATH"
echo "SSH key: $SSH_KEY_EXPANDED"

# Create rsync command with exclusions from .rsync-exclude file
rsync -avz --delete \
    -e "ssh -p $DEPLOY_SSH_PORT -i $SSH_KEY_EXPANDED" \
    --exclude-from='.rsync-exclude' \
    ./ \
    $DEPLOY_SSH_USER@$DEPLOY_SSH_HOST:$DEPLOY_REMOTE_PATH

if [ $? -ne 0 ]; then
    echo "Rsync failed!"
    exit 1
fi

echo "Executing remote commands..."
ssh -p $DEPLOY_SSH_PORT -i $SSH_KEY_EXPANDED $DEPLOY_SSH_USER@$DEPLOY_SSH_HOST << EOF
    set -e

    cd $DEPLOY_REMOTE_PATH

    echo "Building and starting Docker containers..."
    docker compose up --build -d

    echo "Waiting for container to start..."
    sleep 5

    echo "Container status:"
    docker compose ps

    # Check if the container is running
    if docker compose ps --status running | grep -q faint_light; then
        echo "faint_light container is running!"
    else
        echo "Container failed to start. Logs:"
        docker compose logs --tail 30
        exit 1
    fi
EOF

if [ $? -eq 0 ]; then
    echo "Deployment and remote execution completed successfully!"
else
    echo "Remote execution failed!"
    exit 1
fi
