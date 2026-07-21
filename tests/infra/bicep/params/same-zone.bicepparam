// same-zone.bicepparam
// Both benchmark nodes in the SAME availability zone, same VNet/subnet, region
// co-located via a Proximity Placement Group for minimal latency.
//
// Secrets are NOT hard-coded: the SSH public key and admin CIDR are read from
// environment variables at deploy time (scripts/deploy.sh sets them). The
// defaults below are inert placeholders so the file validates without secrets;
// 0.0.0.0/32 grants no SSH access, forcing an explicit admin CIDR.
using '../main.bicep'

param topology = 'same-zone'
param location = 'eastus2'
param primaryZone = '1'
param vmSize = 'Standard_D2s_v5'
param adminUsername = 'azureuser'
param benchPorts = [
  '50051'
]
param enablePublicIpForSsh = true

param sshPublicKey = readEnvironmentVariable('TONICH3_SSH_PUBKEY', 'ssh-ed25519 AAAA_REPLACE_ME_placeholder')
param adminSourceCidr = readEnvironmentVariable('TONICH3_ADMIN_CIDR', '0.0.0.0/32')
