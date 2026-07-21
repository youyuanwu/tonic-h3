// cross-region.bicepparam
// Benchmark nodes in TWO regions, each in its own VNet, joined by GLOBAL VNet
// peering so traffic stays on the Azure backbone (private IPs, no public
// transit). The two VNet address spaces must not overlap.
//
// Secrets are NOT hard-coded: the SSH public key and admin CIDR are read from
// environment variables at deploy time (scripts/deploy.sh sets them). The
// defaults below are inert placeholders so the file validates without secrets;
// 0.0.0.0/32 grants no SSH access, forcing an explicit admin CIDR.
using '../main.bicep'

param topology = 'cross-region'
param location = 'eastus2'
param secondaryLocation = 'westus2'
param primaryZone = '1'
param vmSize = 'Standard_D2s_v5'
param adminUsername = 'azureuser'
param benchPorts = [
  '50051'
]
param primaryVnetAddressPrefix = '10.20.0.0/16'
param primarySubnetPrefix = '10.20.1.0/24'
param secondaryVnetAddressPrefix = '10.30.0.0/16'
param secondarySubnetPrefix = '10.30.1.0/24'
param enablePublicIpForSsh = true

param sshPublicKey = readEnvironmentVariable('TONICH3_SSH_PUBKEY', 'ssh-ed25519 AAAA_REPLACE_ME_placeholder')
param adminSourceCidr = readEnvironmentVariable('TONICH3_ADMIN_CIDR', '0.0.0.0/32')
