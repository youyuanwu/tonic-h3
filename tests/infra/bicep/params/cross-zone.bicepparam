// cross-zone.bicepparam
// Benchmark nodes in DIFFERENT availability zones of the SAME region, sharing a
// single VNet/subnet. No Proximity Placement Group (zones are distinct).
//
// Secrets are NOT hard-coded: the SSH public key and admin CIDR are read from
// environment variables at deploy time (scripts/deploy.sh sets them). The
// defaults below are inert placeholders so the file validates without secrets;
// 0.0.0.0/32 grants no SSH access, forcing an explicit admin CIDR.
using '../main.bicep'

param topology = 'cross-zone'
param location = 'eastus2'
param primaryZone = '1'
param secondaryZone = '2'
param vmSize = 'Standard_D4s_v5'
param adminUsername = 'azureuser'
param benchPorts = [
  '50051'
]
param enablePublicIpForSsh = true
param enableCloudInit = false

param sshPublicKey = readEnvironmentVariable('TONICH3_SSH_PUBKEY', 'ssh-ed25519 AAAA_REPLACE_ME_placeholder')
param adminSourceCidr = readEnvironmentVariable('TONICH3_ADMIN_CIDR', '0.0.0.0/32')
