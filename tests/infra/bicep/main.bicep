// main.bicep
// Benchmark harness infrastructure for tonic-h3 (gRPC over HTTP/3 vs HTTP/2).
//
// Scope: RESOURCE GROUP. The resource group is created by scripts/deploy.sh
// (`az group create`) before this template is deployed. A single deployment
// provisions ONE client + ONE server benchmark node wired for one of three
// network topologies, selected via the `topology` parameter:
//
//   same-zone     both nodes in the SAME zone, same VNet/subnet, co-located
//                 with a Proximity Placement Group (minimal latency).
//   cross-zone    nodes in DIFFERENT zones, same region, same VNet.
//   cross-region  nodes in two regions, two VNets, joined by GLOBAL VNet
//                 peering so traffic stays on the Azure backbone (private IPs).
//
// In every topology the benchmark data plane uses PRIVATE IPs only and never
// touches the public internet (see modules/network.bicep for the NSG rules).
// For cross-region, both regions' resources live in this one resource group so
// that `az group delete` tears the whole scenario down in a single command.

targetScope = 'resourceGroup'

@description('Network topology to provision.')
@allowed([
  'same-zone'
  'cross-zone'
  'cross-region'
])
param topology string = 'same-zone'

@description('Primary region. Must support Availability Zones (e.g. eastus2).')
param location string = resourceGroup().location

@description('Secondary region for the cross-region topology. Must support AZ + global peering (e.g. westus2).')
param secondaryLocation string = 'westus2'

@description('Resource name prefix.')
param namePrefix string = 'tonich3'

@description('Unique-ish suffix to disambiguate resource names across deployments.')
param deploySuffix string = take(uniqueString(resourceGroup().id, topology), 6)

@description('VM size. Must support Accelerated Networking (D/Ds v5-class recommended).')
param vmSize string = 'Standard_D4s_v5'

@description('Admin username for the Linux VMs.')
param adminUsername string = 'azureuser'

@description('SSH public key (OpenSSH format). Supplied at deploy time; never hard-coded in source.')
@minLength(1)
param sshPublicKey string

@description('Source CIDR allowed to reach management SSH (22/TCP). Use a /32 or a small admin range.')
param adminSourceCidr string

@description('Benchmark destination port range(s), applied to BOTH TCP (HTTP/2) and UDP (HTTP/3/QUIC).')
param benchPorts array = [
  '50051'
]

@description('Zone for the client node (and the server node except in cross-zone).')
param primaryZone string = '1'

@description('Zone for the server node in the cross-zone topology.')
param secondaryZone string = '2'

@description('Address space for the primary VNet.')
param primaryVnetAddressPrefix string = '10.20.0.0/16'

@description('Benchmark subnet prefix within the primary VNet.')
param primarySubnetPrefix string = '10.20.1.0/24'

@description('Address space for the secondary VNet (cross-region only). Must not overlap the primary VNet.')
param secondaryVnetAddressPrefix string = '10.30.0.0/16'

@description('Benchmark subnet prefix within the secondary VNet (cross-region only).')
param secondarySubnetPrefix string = '10.30.1.0/24'

@description('Attach a management public IP to each VM for SSH. Disable to use Azure Bastion instead.')
param enablePublicIpForSsh bool = true

@description('Run the optional cloud-init stub (installs build prerequisites) on each node.')
param enableCloudInit bool = false

@description('Resource tags applied to every resource.')
param tags object = {
  workload: 'tonic-h3-bench'
  topology: topology
}

// ---------------------------------------------------------------------------
// Derived values
// ---------------------------------------------------------------------------
var isSameZone = topology == 'same-zone'
var isCrossZone = topology == 'cross-zone'
var isCrossRegion = topology == 'cross-region'

var subnetName = 'bench'

var primaryNodeBase = '${namePrefix}-primary-${deploySuffix}'
var secondaryNodeBase = '${namePrefix}-secondary-${deploySuffix}'
var primaryVnetName = '${primaryNodeBase}-vnet'
var secondaryVnetName = '${secondaryNodeBase}-vnet'

var clientVmName = '${namePrefix}-client-${deploySuffix}'
var serverVmName = '${namePrefix}-server-${deploySuffix}'

var ppgName = '${namePrefix}-ppg-${deploySuffix}'

// Location / zone selection per topology.
var serverLocation = isCrossRegion ? secondaryLocation : location
var clientZone = primaryZone
var serverZone = isCrossZone ? secondaryZone : primaryZone

// PPG id as a deterministic string (only meaningful for same-zone). Using
// resourceId() rather than a conditional-resource reference keeps the VM module
// inputs free of possible-null (BCP318) warnings; ordering is guaranteed via the
// explicit dependsOn on the ppg resource below.
var ppgId = isSameZone ? resourceId('Microsoft.Compute/proximityPlacementGroups', ppgName) : ''

// Server subnet id: primary subnet for same/cross-zone, secondary subnet for
// cross-region. The cross-region branch is a pure string (no conditional module
// reference); the dependsOn on networkSecondary guarantees the subnet exists.
var serverSubnetId = isCrossRegion
  ? resourceId('Microsoft.Network/virtualNetworks/subnets', secondaryVnetName, subnetName)
  : networkPrimary.outputs.subnetId

var customDataBase64 = enableCloudInit ? loadFileAsBase64('cloud-init/bench-node.yaml') : ''

// ---------------------------------------------------------------------------
// Proximity Placement Group (same-zone only, for minimal latency co-location)
// ---------------------------------------------------------------------------
resource ppg 'Microsoft.Compute/proximityPlacementGroups@2023-09-01' = if (isSameZone) {
  name: ppgName
  location: location
  tags: tags
  properties: {
    proximityPlacementGroupType: 'Standard'
  }
}

// ---------------------------------------------------------------------------
// Networking
// ---------------------------------------------------------------------------
module networkPrimary 'modules/network.bicep' = {
  name: 'network-primary'
  params: {
    name: primaryNodeBase
    location: location
    vnetAddressPrefix: primaryVnetAddressPrefix
    subnetPrefix: primarySubnetPrefix
    subnetName: subnetName
    adminSourceCidr: adminSourceCidr
    benchPorts: benchPorts
    tags: tags
  }
}

module networkSecondary 'modules/network.bicep' = if (isCrossRegion) {
  name: 'network-secondary'
  params: {
    name: secondaryNodeBase
    location: secondaryLocation
    vnetAddressPrefix: secondaryVnetAddressPrefix
    subnetPrefix: secondarySubnetPrefix
    subnetName: subnetName
    adminSourceCidr: adminSourceCidr
    benchPorts: benchPorts
    tags: tags
  }
}

module peering 'modules/peering.bicep' = if (isCrossRegion) {
  name: 'peering-global'
  params: {
    primaryVnetName: primaryVnetName
    secondaryVnetName: secondaryVnetName
  }
  dependsOn: [
    networkPrimary
    networkSecondary
  ]
}

// ---------------------------------------------------------------------------
// Benchmark nodes
// ---------------------------------------------------------------------------
module clientVm 'modules/vm.bicep' = {
  name: 'vm-client'
  params: {
    name: clientVmName
    location: location
    zone: clientZone
    subnetId: networkPrimary.outputs.subnetId
    vmSize: vmSize
    adminUsername: adminUsername
    sshPublicKey: sshPublicKey
    customDataBase64: customDataBase64
    ppgId: ppgId
    enablePublicIp: enablePublicIpForSsh
    tags: tags
  }
  dependsOn: [
    ppg
  ]
}

module serverVm 'modules/vm.bicep' = {
  name: 'vm-server'
  params: {
    name: serverVmName
    location: serverLocation
    zone: serverZone
    subnetId: serverSubnetId
    vmSize: vmSize
    adminUsername: adminUsername
    sshPublicKey: sshPublicKey
    customDataBase64: customDataBase64
    ppgId: ppgId
    enablePublicIp: enablePublicIpForSsh
    tags: tags
  }
  dependsOn: [
    ppg
    networkSecondary
  ]
}

// ---------------------------------------------------------------------------
// Outputs (consumed by the benchmark harness)
// ---------------------------------------------------------------------------
@description('Selected topology.')
output topology string = topology

@description('Resource group holding the scenario.')
output resourceGroupName string = resourceGroup().name

@description('Client node region.')
output clientLocation string = location

@description('Server node region.')
output serverLocation string = serverLocation

@description('Client node availability zone.')
output clientZone string = clientZone

@description('Server node availability zone.')
output serverZone string = serverZone

@description('PRIVATE IP of the client node (harness load driver source).')
output clientPrivateIp string = clientVm.outputs.privateIp

@description('PRIVATE IP of the server node (benchmark target).')
output serverPrivateIp string = serverVm.outputs.privateIp

@description('Management public IP of the client node (empty when Bastion mode).')
output clientSshPublicIp string = clientVm.outputs.publicIp

@description('Management public IP of the server node (empty when Bastion mode).')
output serverSshPublicIp string = serverVm.outputs.publicIp

@description('Management FQDN of the client node (empty when Bastion mode).')
output clientSshFqdn string = clientVm.outputs.fqdn

@description('Management FQDN of the server node (empty when Bastion mode).')
output serverSshFqdn string = serverVm.outputs.fqdn

@description('Benchmark port range(s) opened for TCP + UDP within the VNet.')
output benchPorts array = benchPorts
