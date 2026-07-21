// network.bicep
// Creates one VNet with a single benchmark subnet and an attached NSG.
//
// Security posture:
//   * SSH (22/TCP) is allowed ONLY from the parameterised admin CIDR.
//   * Benchmark ports (TCP + UDP) are allowed ONLY from the `VirtualNetwork`
//     service tag. That tag resolves to this VNet's address space AND the
//     address space of any *peered* VNets, so the same rule set keeps
//     cross-region (globally peered) traffic private without hard-coding the
//     remote prefixes.
//   * An explicit low-priority rule denies the benchmark ports from the
//     `Internet` tag, on top of Azure's default `DenyAllInBound`, to make the
//     "never exposed publicly" intent unambiguous.

@description('Base name for the VNet/NSG/subnet resources (a suffix is appended per resource type).')
param name string

@description('Azure region for this VNet.')
param location string

@description('CIDR for the VNet address space, e.g. 10.20.0.0/16. Must not overlap the peer VNet.')
param vnetAddressPrefix string

@description('CIDR for the benchmark subnet, e.g. 10.20.1.0/24.')
param subnetPrefix string

@description('Name of the benchmark subnet.')
param subnetName string = 'bench'

@description('Source CIDR permitted to reach SSH (22/TCP). Use a /32 admin IP or a jumpbox range.')
param adminSourceCidr string

@description('Destination port ranges for benchmark traffic. Applied to BOTH a TCP and a UDP allow rule.')
param benchPorts array

@description('Resource tags.')
param tags object = {}

resource nsg 'Microsoft.Network/networkSecurityGroups@2023-11-01' = {
  name: '${name}-nsg'
  location: location
  tags: tags
  properties: {
    securityRules: [
      {
        name: 'AllowSshInbound'
        properties: {
          priority: 100
          direction: 'Inbound'
          access: 'Allow'
          protocol: 'Tcp'
          sourceAddressPrefix: adminSourceCidr
          sourcePortRange: '*'
          destinationAddressPrefix: '*'
          destinationPortRange: '22'
          description: 'Management SSH from the admin CIDR only.'
        }
      }
      {
        name: 'AllowBenchTcpInbound'
        properties: {
          priority: 200
          direction: 'Inbound'
          access: 'Allow'
          protocol: 'Tcp'
          sourceAddressPrefix: 'VirtualNetwork'
          sourcePortRange: '*'
          destinationAddressPrefix: 'VirtualNetwork'
          destinationPortRanges: benchPorts
          description: 'gRPC over HTTP/2 (TCP+TLS) benchmark traffic from the VNet / peered VNets only.'
        }
      }
      {
        name: 'AllowBenchUdpInbound'
        properties: {
          priority: 210
          direction: 'Inbound'
          access: 'Allow'
          protocol: 'Udp'
          sourceAddressPrefix: 'VirtualNetwork'
          sourcePortRange: '*'
          destinationAddressPrefix: 'VirtualNetwork'
          destinationPortRanges: benchPorts
          description: 'gRPC over HTTP/3 (QUIC/UDP) benchmark traffic from the VNet / peered VNets only.'
        }
      }
      {
        name: 'DenyBenchFromInternetInbound'
        properties: {
          priority: 400
          direction: 'Inbound'
          access: 'Deny'
          protocol: '*'
          sourceAddressPrefix: 'Internet'
          sourcePortRange: '*'
          destinationAddressPrefix: '*'
          destinationPortRanges: benchPorts
          description: 'Belt-and-braces: benchmark ports are never reachable from the public internet.'
        }
      }
    ]
  }
}

resource vnet 'Microsoft.Network/virtualNetworks@2023-11-01' = {
  name: '${name}-vnet'
  location: location
  tags: tags
  properties: {
    addressSpace: {
      addressPrefixes: [
        vnetAddressPrefix
      ]
    }
    subnets: [
      {
        name: subnetName
        properties: {
          addressPrefix: subnetPrefix
          networkSecurityGroup: {
            id: nsg.id
          }
        }
      }
    ]
  }
}

@description('Resource ID of the VNet.')
output vnetId string = vnet.id

@description('Name of the VNet.')
output vnetName string = vnet.name

@description('Resource ID of the benchmark subnet.')
output subnetId string = vnet.properties.subnets[0].id
